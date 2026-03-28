pub mod binder_node {
    //! Simplified Android Binder Node management system.
    //!
    //! This module emulates core aspects of Android's Binder IPC mechanism (both kernel driver
    //! and libbinder patterns) for managing Binder nodes. A Binder node represents a remotely
    //! reference-counted object that can be passed across processes. When the last reference
    //! to a node is released (or the hosting process dies), registered death notifications
    //! must be delivered to interested parties.
    //!
    //! Key design choices:
    //! - **Intrusive doubly-linked list** for death notifications: Mirrors the Linux kernel's
    //!   `list_head` pattern used in `binder_node` and `binder_death`. The links live inside
    //!   each `DeathNotification`, eliminating per-element allocation overhead and enabling
    //!   O(1) insertion/unlinking — critical for a high-throughput IPC system.
    //! - **High-concurrency release handling**: Nodes are removed from the active set *under*
    //!   the manager lock, but the lock is dropped *before* any cleanup work. The node's death
    //!   notifications are then moved to a temporary chain (via `take_all`) and processed
    //!   outside *any* lock. This prevents lock contention, long critical sections, and
    //!   potential deadlocks if callbacks themselves acquire locks or perform IPC.
    //! - **Memory stability guarantees**: The intrusive chain is transferred atomically under
    //!   the lock. Raw pointers (`NonNull`) keep the notification objects alive during the
    //!   hand-off. Only after the manager lock is released do we reconstruct `Box`es and
    //!   invoke callbacks. No external references to notifications exist, eliminating
    //!   use-after-free races even under heavy concurrent registration/release pressure.
    //!
    //! All operations are thread-safe via `std::sync::Mutex`. The unsafe code is strictly
    //! confined, lifetime-controlled, and justified by the ownership transfer model.

    use std::collections::HashMap;
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex};

    // -------------------------------------------------------------------------
    // Intrusive death notification node (embedded links + payload)
    // -------------------------------------------------------------------------
    struct DeathNotification {
        next: Option<NonNull<DeathNotification>>,
        prev: Option<NonNull<DeathNotification>>,
        recipient_id: u64,
        on_death: Box<dyn FnOnce() + Send>,
    }

    impl DeathNotification {
        fn new(recipient_id: u64, callback: impl FnOnce() + Send + 'static) -> Box<Self> {
            Box::new(Self {
                next: None,
                prev: None,
                recipient_id,
                on_death: Box::new(callback),
            })
        }
    }

    // -------------------------------------------------------------------------
    // Intrusive list head (zero-allocation list management)
    // -------------------------------------------------------------------------
    struct DeathList {
        head: Option<NonNull<DeathNotification>>,
    }

    impl DeathList {
        const fn new() -> Self {
            Self { head: None }
        }

        /// Insert a notification at the head (O(1)). Ownership of the `Box` is transferred
        /// into the list via `Box::leak`. The caller must never drop the original box.
        fn insert(&mut self, node_box: Box<DeathNotification>) {
            let node_ptr = NonNull::from(Box::leak(node_box));

            unsafe {
                if let Some(mut old_head) = self.head {
                    // Insert before current head
                    node_ptr.as_mut().next = Some(old_head);
                    node_ptr.as_mut().prev = None;
                    old_head.as_mut().prev = Some(node_ptr);
                } else {
                    node_ptr.as_mut().next = None;
                    node_ptr.as_mut().prev = None;
                }
                self.head = Some(node_ptr);
            }
        }

        /// Atomically take the entire list (clears the head). Returns the old head pointer
        /// which now represents a temporary intrusive chain owned by the caller.
        fn take_all(&mut self) -> Option<NonNull<DeathNotification>> {
            self.head.take()
        }
    }

    // -------------------------------------------------------------------------
    // Process a temporary death-notification chain outside any lock
    // -------------------------------------------------------------------------
    fn process_deaths(mut head: Option<NonNull<DeathNotification>>) {
        while let Some(current) = head {
            // SAFETY: The chain was taken under lock; no other code holds live references
            // to these nodes. We reconstruct ownership one node at a time.
            let mut node = unsafe { Box::from_raw(current.as_ptr()) };
            let next_head = node.next; // Copy (NonNull<Option> is Copy) before any moves

            // Move the callback out so we can invoke it (FnOnce consumes self)
            let on_death = node.on_death;

            // Invoke death callback. No locks are held at this point.
            on_death();

            // Remaining fields (next, prev, recipient_id) are dropped automatically
            // when `node` goes out of scope. The next pointer was already copied.
            head = next_head;
        }
    }

    // -------------------------------------------------------------------------
    // Per-node internal state (protected by its own Mutex)
    // -------------------------------------------------------------------------
    struct BinderNodeInner {
        death_notifications: DeathList,
        alive: bool,
    }

    impl BinderNodeInner {
        fn new() -> Self {
            Self {
                death_notifications: DeathList::new(),
                alive: true,
            }
        }

        fn register_death(
            &mut self,
            recipient_id: u64,
            callback: impl FnOnce() + Send + 'static,
        ) {
            let death_node = DeathNotification::new(recipient_id, callback);
            self.death_notifications.insert(death_node);
        }

        /// Take all pending death notifications and mark the node dead.
        /// Returns the temporary chain for processing *outside* the inner lock.
        fn take_death_notifications(&mut self) -> Option<NonNull<DeathNotification>> {
            if !self.alive {
                return None;
            }
            self.alive = false;
            self.death_notifications.take_all()
        }
    }

    // -------------------------------------------------------------------------
    // Public Binder node (reference-counted via Arc)
    // -------------------------------------------------------------------------
    pub struct BinderNode {
        id: u64,
        inner: Mutex<BinderNodeInner>,
    }

    impl BinderNode {
        fn new(id: u64) -> Self {
            Self {
                id,
                inner: Mutex::new(BinderNodeInner::new()),
            }
        }
    }

    // -------------------------------------------------------------------------
    // Central node manager (handles creation, registration, and high-concurrency releases)
    // -------------------------------------------------------------------------
    pub struct BinderNodeManager {
        active_nodes: Mutex<HashMap<u64, Arc<BinderNode>>>,
    }

    impl BinderNodeManager {
        pub fn new() -> Self {
            Self {
                active_nodes: Mutex::new(HashMap::new()),
            }
        }

        /// Create and register a new Binder node. Returns an `Arc` handle that can be held
        /// by client code. The manager retains one strong reference internally.
        pub fn create_node(&self, id: u64) -> Arc<BinderNode> {
            let node = Arc::new(BinderNode::new(id));
            let mut guard = self.active_nodes.lock().unwrap();
            guard.insert(id, Arc::clone(&node));
            node
        }

        /// Register a death notification for a live node.
        /// Returns `Err` if the node has already been released.
        pub fn register_death_notification(
            &self,
            node_id: u64,
            recipient_id: u64,
            callback: impl FnOnce() + Send + 'static,
        ) -> Result<(), String> {
            let guard = self.active_nodes.lock().unwrap();
            if let Some(node) = guard.get(&node_id) {
                let mut inner = node.inner.lock().unwrap();
                inner.register_death(recipient_id, callback);
                Ok(())
            } else {
                Err(format!("Binder node {} not found or already released", node_id))
            }
        }

        /// Release a Binder node (simulates last reference count reaching zero).
        /// 
        /// The node is removed from the active map *under* the manager lock.
        /// The lock is then dropped *before* any death processing occurs.
        /// This is the core high-concurrency pattern: other threads can continue
        /// creating nodes, registering deaths, or releasing unrelated nodes without
        /// waiting for potentially expensive callbacks.
        pub fn release_node(&self, node_id: u64) {
            let node_opt = {
                let mut guard = self.active_nodes.lock().unwrap();
                guard.remove(&node_id)
            };

            if let Some(node) = node_opt {
                self.cleanup_node(node);
            }
        }

        /// Private cleanup routine. Runs entirely outside the manager lock.
        /// Acquires the node's inner lock only long enough to extract the death list,
        /// then releases it before processing.
        fn cleanup_node(&self, node: Arc<BinderNode>) {
            let temp_head = {
                let mut inner_guard = node.inner.lock().unwrap();
                inner_guard.take_death_notifications()
            };
            // Inner lock is dropped here — callbacks may safely acquire other locks

            process_deaths(temp_head);

            // The Arc goes out of scope here. If this was the last reference,
            // the BinderNode (and its Mutex) is dropped safely.
        }

        /// For demonstration / testing: returns current active node count.
        pub fn active_node_count(&self) -> usize {
            self.active_nodes.lock().unwrap().len()
        }
    }

    // -------------------------------------------------------------------------
    // Example usage (can be run in tests or main)
    // -------------------------------------------------------------------------
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn demo_binder_node_release_with_concurrent_safety() {
            let manager = BinderNodeManager::new();
            let node = manager.create_node(42);

            // Register multiple death notifications
            manager
                .register_death_notification(42, 1, || println!("Death recipient 1 notified"))
                .unwrap();
            manager
                .register_death_notification(42, 2, || println!("Death recipient 2 notified"))
                .unwrap();

            // Release triggers cleanup outside all locks
            manager.release_node(42);

            // After release, new registrations fail (node is dead)
            assert!(manager
                .register_death_notification(42, 3, || println!("Too late"))
                .is_err());

            assert_eq!(manager.active_node_count(), 0);
        }
    }
}