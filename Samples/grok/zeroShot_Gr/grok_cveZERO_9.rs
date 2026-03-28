use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::ptr::{self, NonNull};
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}, Mutex, RwLock};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

pub mod binder_node {
    use super::*;

    #[derive(Debug)]
    pub struct DeathLink {
        next: UnsafeCell<Option<NonNull<DeathNotification>>>,
        prev: UnsafeCell<Option<NonNull<DeathNotification>>>,
    }

    unsafe impl Send for DeathLink {}
    unsafe impl Sync for DeathLink {}

    impl DeathLink {
        #[inline]
        pub const fn new() -> Self {
            Self {
                next: UnsafeCell::new(None),
                prev: UnsafeCell::new(None),
            }
        }
    }

    #[derive(Debug)]
    pub struct DeathNotification {
        pub cookie: u64,
        pub link: DeathLink,
    }

    impl DeathNotification {
        pub fn new(cookie: u64) -> Arc<Self> {
            Arc::new(Self {
                cookie,
                link: DeathLink::new(),
            })
        }
    }

    #[derive(Debug)]
    struct BinderNodeInner {
        strong_refs: AtomicUsize,
        weak_refs: AtomicUsize,
        death_lock: Mutex<()>,
        death_head: UnsafeCell<*mut DeathNotification>,
        death_tail: UnsafeCell<*mut DeathNotification>,
        death_count: AtomicUsize,
    }

    unsafe impl Send for BinderNodeInner {}
    unsafe impl Sync for BinderNodeInner {}

    impl BinderNodeInner {
        fn new() -> Self {
            Self {
                strong_refs: AtomicUsize::new(1), // initial strong ref
                weak_refs: AtomicUsize::new(0),
                death_lock: Mutex::new(()),
                death_head: UnsafeCell::new(ptr::null_mut()),
                death_tail: UnsafeCell::new(ptr::null_mut()),
                death_count: AtomicUsize::new(0),
            }
        }

        #[inline]
        unsafe fn push_back_death_raw(&mut self, ptr: *mut DeathNotification) {
            // Assume caller has validated ptr is valid and not already linked
            (*ptr).link.next.get().write(None);
            (*ptr).link.prev.get().write(None);

            let tail_ptr = *self.death_tail.get();
            if tail_ptr.is_null() {
                *self.death_head.get() = ptr;
                *self.death_tail.get() = ptr;
            } else {
                (*tail_ptr).link.next.get().write(Some(NonNull::new(ptr).unwrap()));
                (*ptr).link.prev.get().write(Some(NonNull::new(tail_ptr).unwrap()));
                *self.death_tail.get() = ptr;
            }
        }

        #[inline]
        unsafe fn remove_death_raw(&mut self, ptr: *mut DeathNotification) {
            // Standard doubly-linked list removal
            let next_opt = (*ptr).link.next.get().read();
            let prev_opt = (*ptr).link.prev.get().read();

            if let Some(next) = next_opt {
                let next_ptr = next.as_ptr();
                (*next_ptr).link.prev.get().write(prev_opt);
            } else {
                // Was tail
                *self.death_tail.get() = prev_opt.map(|p| p.as_ptr()).unwrap_or(ptr::null_mut());
            }

            if let Some(prev) = prev_opt {
                let prev_ptr = prev.as_ptr();
                (*prev_ptr).link.next.get().write(next_opt);
            } else {
                // Was head
                *self.death_head.get() = next_opt.map(|n| n.as_ptr()).unwrap_or(ptr::null_mut());
            }

            // Clear links
            (*ptr).link.next.get().write(None);
            (*ptr).link.prev.get().write(None);
        }

        unsafe fn for_each_death<F>(&self, mut f: F)
        where
            F: FnMut(u64),
        {
            let _guard = self.death_lock.lock().unwrap();
            let mut cur_ptr = *self.death_head.get();
            while !cur_ptr.is_null() {
                let cookie = (*cur_ptr).cookie;
                f(cookie);

                // Get next before potential modification (though notify shouldn't remove)
                let next_opt = (*cur_ptr).link.next.get().read();
                cur_ptr = next_opt.map_or(ptr::null_mut(), |nn| nn.as_ptr());
            }
        }
    }

    #[derive(Debug, Clone)]
    pub struct BinderNode {
        pub id: u64,
        inner: Arc<Mutex<BinderNodeInner>>,
    }

    impl BinderNode {
        pub fn new(id: u64) -> Self {
            Self {
                id,
                inner: Arc::new(Mutex::new(BinderNodeInner::new())),
            }
        }

        pub fn inc_strong(&self) {
            let mut guard = self.inner.lock().unwrap();
            guard.strong_refs.fetch_add(1, Ordering::AcqRel);
        }

        pub fn dec_strong(&self, manager: &BinderNodeManager) {
            let mut guard = self.inner.lock().unwrap();
            let prev = guard.strong_refs.fetch_sub(1, Ordering::AcqRel);
            if prev == 1 {
                // Notify death recipients
                unsafe {
                    guard.for_each_death(|cookie| {
                        // Simplified: in real impl, invoke callback with cookie
                        // e.g., log or send to recipient process
                        tracing::debug!("Death notification for node {} cookie {}", self.id, cookie);
                    });
                }
                // Clear the death list (optional, as refs should unlink before drop)
                let _dguard = guard.death_lock.lock().unwrap();
                *guard.death_head.get() = ptr::null_mut();
                *guard.death_tail.get() = ptr::null_mut();
                guard.death_count.store(0, Ordering::Relaxed);
                drop(guard);
                // Defer drop/clone to temp list
                manager.schedule_for_cleanup(self.clone());
            }
        }

        pub fn inc_weak(&self) {
            let mut guard = self.inner.lock().unwrap();
            guard.weak_refs.fetch_add(1, Ordering::AcqRel);
        }

        pub fn dec_weak(&self) {
            let mut guard = self.inner.lock().unwrap();
            let prev = guard.weak_refs.fetch_sub(1, Ordering::AcqRel);
            if prev == 1 && guard.strong_refs.load(Ordering::Relaxed) == 0 {
                // Final cleanup if no strong refs
                drop(guard);
                // In real impl, free resources like descriptors
            }
        }

        pub fn add_death_recipient(&self, notif: Arc<DeathNotification>) {
            let ptr = Arc::as_ptr(&notif) as *mut DeathNotification;
            let mut guard = self.inner.lock().unwrap();
            let _dguard = guard.death_lock.lock().unwrap();
            unsafe {
                guard.push_back_death_raw(ptr);
            }
            guard.death_count.fetch_add(1, Ordering::Relaxed);
        }

        pub fn remove_death_recipient(&self, notif: &Arc<DeathNotification>) {
            let ptr = Arc::as_ptr(notif) as *mut DeathNotification;
            let mut guard = self.inner.lock().unwrap();
            let _dguard = guard.death_lock.lock().unwrap();
            unsafe {
                guard.remove_death_raw(ptr);
            }
            guard.death_count.fetch_sub(1, Ordering::Relaxed);
        }

        pub fn death_count(&self) -> usize {
            self.inner.lock().unwrap().death_count.load(Ordering::Relaxed)
        }
    }
}

use binder_node::*;

#[derive(Debug)]
pub struct BinderNodeManager {
    /// Sharded cleanup queues to handle high-concurrency releases with low contention
    cleanup_queues: Vec<Mutex<VecDeque<Arc<BinderNode>>>>,
    /// Optional: track live nodes by ID for lookups/resurrection checks
    live_nodes: RwLock<HashMap<u64, Weak<BinderNode>>>,
    next_id: AtomicUsize,
}

impl BinderNodeManager {
    pub fn new(num_cleanup_shards: usize) -> Self {
        let mut queues = Vec::with_capacity(num_cleanup_shards);
        for _ in 0..num_cleanup_shards {
            queues.push(Mutex::new(VecDeque::new()));
        }
        Self {
            cleanup_queues: queues,
            live_nodes: RwLock::new(HashMap::new()),
            next_id: AtomicUsize::new(1),
        }
    }

    pub fn create_node(&self) -> Arc<BinderNode> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) as u64;
        let node = Arc::new(BinderNode::new(id));
        self.live_nodes
            .write()
            .unwrap()
            .insert(id, Arc::downgrade(&node));
        node
    }

    /// Schedule node for deferred cleanup (extra Arc ref keeps it alive)
    pub fn schedule_for_cleanup(&self, node: Arc<BinderNode>) {
        let shard_idx = {
            let mut hasher = DefaultHasher::new();
            node.id.hash(&mut hasher);
            (hasher.finish() as usize) % self.cleanup_queues.len()
        };
        self.cleanup_queues[shard_idx]
            .lock()
            .unwrap()
            .push_back(node);
    }

    /// Process all cleanup queues (call periodically or from a background thread)
    /// Drops the extra ref, triggering finalization if no other holders remain.
    pub fn process_cleanup(&self) {
        for (i, queue) in self.cleanup_queues.iter().enumerate() {
            let mut guard = queue.lock().unwrap();
            let batch: Vec<_> = guard.drain(..).collect();
            drop(guard);
            for node in batch {
                // Final weak dec if needed, remove from live_nodes
                node.dec_weak();
                self.live_nodes
                    .write()
                    .unwrap()
                    .remove(&node.id);
                // Arc drop happens naturally here
            }
            tracing::debug!("Processed cleanup shard {}", i);
        }
    }

    pub fn get_node(&self, id: u64) -> Option<Arc<BinderNode>> {
        let nodes = self.live_nodes.read().unwrap();
        nodes.get(&id)
            .and_then(|w| w.upgrade())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_refcount_and_death_list() {
        let mgr = BinderNodeManager::new(4);
        let node = mgr.create_node();

        // Test death recipient
        let notif1 = DeathNotification::new(42);
        let notif2 = DeathNotification::new(100);
        node.add_death_recipient(notif1.clone());
        node.add_death_recipient(notif2.clone());
        assert_eq!(node.death_count(), 2);

        node.remove_death_recipient(&notif1);
        assert_eq!(node.death_count(), 1);

        // Test inc/dec strong
        node.inc_strong();
        node.dec_strong(&mgr); // shouldn't cleanup
        node.dec_strong(&mgr); // now strong==0, schedules cleanup but since extra ref, survives

        // Manual process
        mgr.process_cleanup();
    }

    #[test]
    fn test_concurrent_releases() {
        use std::sync::Barrier;
        use std::thread;

        let mgr = BinderNodeManager::new(16);
        let node = Arc::new(mgr.create_node());
        let refs: Vec<_> = (0..100).map(|_| node.clone()).collect();

        let barrier = Arc::new(Barrier::new(100));
        let handles: Vec<_> = refs.into_iter().enumerate().map(|(i, n)| {
            let b = barrier.clone();
            thread::spawn(move || {
                b.wait();
                n.inc_strong();
                n.dec_strong(&mgr);
            })
        }).collect();

        for h in handles {
            h.join().unwrap();
        }

        // All releases concurrent, sharded queues handle without deadlock/contention issues
        mgr.process_cleanup();
    }
}
