use crate::counter::Counter;
use crate::limit::{Limit, Namespace};
use crate::storage::{Storage, StorageErr};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::iter::FromIterator;
use std::sync::RwLock;
use std::time::{Duration, SystemTime};

// This is a storage implementation that can be compiled to WASM. It is very
// similar to the "InMemory" one. The InMemory implementation cannot be used in
// WASM, because it relies on std:time functions. This implementation avoids
// that.

pub trait Clock: Sync + Send {
    fn get_current_time(&self) -> SystemTime;
}

pub struct CacheEntry<V> {
    pub value: V,
    pub expires_at: SystemTime,
}

impl<V: Copy> CacheEntry<V> {
    fn is_expired(&self, current_time: SystemTime) -> bool {
        current_time > self.expires_at
    }
}

pub struct Cache<K: Eq + Hash, V: Copy> {
    pub map: HashMap<K, CacheEntry<V>>,
}

impl<K: Eq + Hash + Clone, V: Copy> Cache<K, V> {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn get(&self, key: &K) -> Option<&CacheEntry<V>> {
        self.map.get(&key)
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut CacheEntry<V>> {
        self.map.get_mut(&key)
    }

    pub fn insert(&mut self, key: &K, value: V, expires_at: SystemTime) {
        self.map
            .insert(key.clone(), CacheEntry { value, expires_at });
    }

    pub fn remove(&mut self, key: &K) {
        self.map.remove(key);
    }

    pub fn get_all(&mut self, current_time: SystemTime) -> Vec<(K, V, SystemTime)> {
        let iterator = self
            .map
            .iter()
            .filter(|(_key, cache_entry)| !cache_entry.is_expired(current_time))
            .map(|(key, cache_entry)| (key.clone(), cache_entry.value, cache_entry.expires_at));

        Vec::from_iter(iterator)
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}

impl<K: Eq + Hash + Clone, V: Copy> Default for Cache<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct WasmStorage {
    limits_for_namespace: RwLock<HashMap<Namespace, HashMap<Limit, HashSet<Counter>>>>,
    pub counters: RwLock<Cache<Counter, i64>>,
    pub clock: Box<dyn Clock>,
}

impl Storage for WasmStorage {
    fn get_namespaces(&self) -> Result<HashSet<Namespace>, StorageErr> {
        Ok(HashSet::from_iter(
            self.limits_for_namespace.read().unwrap().keys().cloned(),
        ))
    }

    fn add_limit(&self, limit: &Limit) -> Result<(), StorageErr> {
        let namespace = limit.namespace();

        let mut limits_for_namespace = self.limits_for_namespace.write().unwrap();

        match limits_for_namespace.get_mut(&namespace) {
            Some(limits) => {
                limits.insert(limit.clone(), HashSet::new());
            }
            None => {
                let mut limits = HashMap::new();
                limits.insert(limit.clone(), HashSet::new());
                limits_for_namespace.insert(namespace.clone(), limits);
            }
        }

        Ok(())
    }

    fn get_limits(&self, namespace: &Namespace) -> Result<HashSet<Limit>, StorageErr> {
        let limits = match self.limits_for_namespace.read().unwrap().get(namespace) {
            Some(limits) => HashSet::from_iter(limits.keys().cloned()),
            None => HashSet::new(),
        };

        Ok(limits)
    }

    fn delete_limit(&self, limit: &Limit) -> Result<(), StorageErr> {
        self.delete_counters_of_limit(limit);

        let mut limits_for_namespace = self.limits_for_namespace.write().unwrap();

        if let Some(counters_by_limit) = limits_for_namespace.get_mut(limit.namespace()) {
            counters_by_limit.remove(limit);

            if counters_by_limit.is_empty() {
                limits_for_namespace.remove(limit.namespace());
            }
        }

        Ok(())
    }

    fn delete_limits(&self, namespace: &Namespace) -> Result<(), StorageErr> {
        self.delete_counters_in_namespace(namespace);
        self.limits_for_namespace.write().unwrap().remove(namespace);
        Ok(())
    }

    fn is_within_limits(&self, counter: &Counter, delta: i64) -> Result<bool, StorageErr> {
        let stored_counters = self.counters.read().unwrap();
        Ok(self.counter_is_within_limits(counter, stored_counters.get(counter), delta))
    }

    fn update_counter(&self, counter: &Counter, delta: i64) -> Result<(), StorageErr> {
        let mut counters = self.counters.write().unwrap();
        self.insert_or_update_counter(&mut counters, counter, delta);
        Ok(())
    }

    fn check_and_update(
        &self,
        counters: &HashSet<&Counter>,
        delta: i64,
    ) -> Result<bool, StorageErr> {
        // This makes the operator of check + update atomic
        let mut stored_counters = self.counters.write().unwrap();

        for counter in counters {
            if !self.counter_is_within_limits(counter, stored_counters.get(counter), delta) {
                return Ok(false);
            }
        }

        for &counter in counters {
            self.insert_or_update_counter(&mut stored_counters, counter, delta)
        }

        Ok(true)
    }

    fn get_counters(&self, namespace: &Namespace) -> Result<HashSet<Counter>, StorageErr> {
        // TODO: optimize to avoid iterating over all of them.

        let counters_with_vals: Vec<Counter> = self
            .counters
            .write()
            .unwrap()
            .get_all(self.clock.get_current_time())
            .iter()
            .filter(|(counter, _, _)| counter.namespace() == namespace)
            .map(|(counter, value, expires_at)| {
                let mut counter_with_val =
                    Counter::new(counter.limit().clone(), counter.set_variables().clone());
                counter_with_val.set_remaining(*value);
                counter_with_val.set_expires_in(
                    expires_at.duration_since(SystemTime::UNIX_EPOCH).unwrap()
                        - self
                            .clock
                            .get_current_time()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap(),
                );
                counter_with_val
            })
            .collect();

        Ok(HashSet::from_iter(counters_with_vals.iter().cloned()))
    }

    fn clear(&self) -> Result<(), StorageErr> {
        self.counters.write().unwrap().clear();
        self.limits_for_namespace.write().unwrap().clear();
        Ok(())
    }
}

impl WasmStorage {
    pub fn new(clock: Box<impl Clock + 'static>) -> Self {
        Self {
            limits_for_namespace: RwLock::new(HashMap::new()),
            counters: RwLock::new(Cache::default()),
            clock,
        }
    }

    pub fn add_counter(&self, counter: &Counter, value: i64, expires_at: SystemTime) {
        self.counters
            .write()
            .unwrap()
            .insert(counter, value, expires_at);
    }

    fn delete_counters_in_namespace(&self, namespace: &Namespace) {
        if let Some(counters_by_limit) = self.limits_for_namespace.read().unwrap().get(namespace) {
            let mut counters = self.counters.write().unwrap();
            for counter in counters_by_limit.values().flatten() {
                counters.remove(counter);
            }
        }
    }

    fn delete_counters_of_limit(&self, limit: &Limit) {
        if let Some(counters_by_limit) = self
            .limits_for_namespace
            .read()
            .unwrap()
            .get(limit.namespace())
        {
            if let Some(counters_of_limit) = counters_by_limit.get(limit) {
                let mut counters = self.counters.write().unwrap();
                for counter in counters_of_limit {
                    counters.remove(counter);
                }
            }
        }
    }

    fn add_counter_limit_association(&self, counter: &Counter) {
        let namespace = counter.limit().namespace();

        if let Some(counters_by_limit) = self
            .limits_for_namespace
            .write()
            .unwrap()
            .get_mut(namespace)
        {
            counters_by_limit
                .get_mut(counter.limit())
                .unwrap()
                .insert(counter.clone());
        }
    }

    fn insert_or_update_counter(
        &self,
        counters: &mut Cache<Counter, i64>,
        counter: &Counter,
        delta: i64,
    ) {
        match counters.get_mut(counter) {
            Some(entry) => {
                if entry.is_expired(self.clock.get_current_time()) {
                    // TODO: remove duplication. "None" branch is identical.
                    counters.insert(
                        counter,
                        counter.max_value() - delta,
                        self.clock.get_current_time() + Duration::from_secs(counter.seconds()),
                    );
                } else {
                    entry.value -= delta;
                }
            }
            None => {
                counters.insert(
                    counter,
                    counter.max_value() - delta,
                    self.clock.get_current_time() + Duration::from_secs(counter.seconds()),
                );

                self.add_counter_limit_association(counter);
            }
        };
    }

    fn counter_is_within_limits(
        &self,
        counter: &Counter,
        cache_entry: Option<&CacheEntry<i64>>,
        delta: i64,
    ) -> bool {
        match cache_entry {
            Some(entry) => {
                if entry.is_expired(self.clock.get_current_time()) {
                    counter.max_value() - delta >= 0
                } else {
                    entry.value - delta >= 0
                }
            }
            None => counter.max_value() - delta >= 0,
        }
    }
}
