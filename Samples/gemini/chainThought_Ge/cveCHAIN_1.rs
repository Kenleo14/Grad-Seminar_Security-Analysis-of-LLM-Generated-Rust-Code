pub mod binder_node {
    //! Fixed Rust implementation of a simplified Android Binder Node management system.
    //!
    //! This version addresses CVE-2025-68260 by maintaining the synchronization
    //! invariant for the entire lifecycle of death-notification list operations.
    //! The lock is never dropped while the list (or any part of it) can still be
    //! mutated by another thread. Instead of a single "move-to-stack" of the
    //! entire chain, we pop one notification at a time, release the lock only
    //! for its callback, then re-acquire for the next pop.

    use std::collections::HashMap;
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex};

    // -------------------------------------------------------------------------
    // Intrusive death notification (same as before)
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
    // Intrusive doubly-linked list with safe pop + unsafe remove under lock
    // -------------------------------------------------------------------------
    struct DeathList {
        head: Option<NonNull<DeathNotification>>,
    }

    impl DeathList {
        const fn new() -> Self {
            Self { head: None }
        }

        fn insert(&mut self, node_box: Box<DeathNotification>) {
            let node_ptr = NonNull::from(Box::leak(node_box));
            unsafe {
                if let Some(mut old_head) = self.head {
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

        /// Pop the front element, returning full ownership of the Box.
        /// This is the only way notifications leave the protected list.
        fn pop_front(&mut self) -> Option<Box<DeathNotification>> {
            let head = self.head.take()?;
            unsafe {
                let node = head.as_mut();
                let next = node.next;
                if let Some(mut n) = next {
                    n.as_mut().prev = None;
                }
                self.head = next;
                node.next = None;
                node.prev = None;
                Some(Box::from_raw(head.as_ptr()))
            }
        }

        /// # Safety
        /// - Caller must hold the node lock.
        /// - The supplied `node` must currently be linked in *this* list.
        unsafe fn remove(&mut self, node: NonNull<DeathNotification>) {
            let node_mut = node.as_mut();
            let prev = node_mut.prev;
            let next = node_mut.next;

            if let Some(p) = prev {
                p.as_mut().next = next;
            } else if self.head == Some(node) {
                self.head = next;
            }
            if let Some(n) = next {
                n.as_mut().prev = prev;
            }
            node_mut.prev = None;
            node_mut.next = None;
        }

        /// Safe public API for unregistration (traverses under lock, then uses the
        /// unsafe remove). Demonstrates the exact unsafe operation that was racy
        /// in the CVE.
        fn remove_by_recipient(&mut self, recipient_id: u64) -> Result<(), String> {
            let mut current_opt = self.head;
            while let Some(current) = current_opt {
                unsafe {
                    let node = current.as_mut();
                    if node.recipient_id == recipient_id {
                        // SAFETY: We hold &mut self (lock is held) and we just
                        // found the node by traversing from the head.
                        self.remove(current);
                        let _ = Box::from_raw(current.as_ptr()); // drop without callback
                        return Ok(());
                    }
                    current_opt = node.next;
                }
            }
            Err(format!("Death notification for recipient {} not found", recipient_id))
        }
    }

    // -------------------------------------------------------------------------
    // Per-node state (no longer needs "alive" flag — map removal protects it)
    // -------------------------------------------------------------------------
    struct BinderNodeInner {
        death_notifications: DeathList,
    }

    impl BinderNodeInner {
        fn new() -> Self {
            Self {
                death_notifications: DeathList::new(),
            }
        }
    }

    // -------------------------------------------------------------------------
    // Public Binder node
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
    // Central manager
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

        pub fn create_node(&self, id: u64) -> Arc<BinderNode> {
            let node = Arc::new(BinderNode::new(id));
            let mut guard = self.active_nodes.lock().unwrap();
            guard.insert(id, Arc::clone(&node));
            node
        }

        pub fn register_death_notification(
            &self,
            node_id: u64,
            recipient_id: u64,
            callback: impl FnOnce() + Send + 'static,
        ) -> Result<(), String> {
            let guard = self.active_nodes.lock().unwrap();
            if let Some(node) = guard.get(&node_id) {
                let mut inner = node.inner.lock().unwrap();
                let death_node = DeathNotification::new(recipient_id, callback);
                inner.death_notifications.insert(death_node);
                Ok(())
            } else {
                Err(format!("Binder node {} not found or already released", node_id))
            }
        }

        /// Unregistration demonstrates the unsafe remove path that was racy in CVE-2025-68260.
        pub fn unregister_death_notification(
            &self,
            node_id: u64,
            recipient_id: u64,
        ) -> Result<(), String> {
            let guard = self.active_nodes.lock().unwrap();
            if let Some(node) = guard.get(&node_id) {
                let mut inner = node.inner.lock().unwrap();
                inner.death_notifications.remove_by_recipient(recipient_id)
            } else {
                Err(format!("Binder node {} not found", node_id))
            }
        }

        pub fn release_node(&self, node_id: u64) {
            let node_opt = {
                let mut guard = self.active_nodes.lock().unwrap();
                guard.remove(&node_id)
            };

            if let Some(node) = node_opt {
                self.cleanup_node(node);
            }
        }

        /// FIXED cleanup routine — the core of the CVE fix.
        /// We never move the entire list out of the locked scope.
        /// Instead we pop one notification at a time, drop the lock for the
        /// callback, then re-acquire for the next pop. This guarantees that
        /// every list mutation (pop or remove) happens while the lock is held.
        fn cleanup_node(&self, node: Arc<BinderNode>) {
            let inner_mutex = &node.inner;

            loop {
                let death_opt = {
                    let mut guard = inner_mutex.lock().unwrap();
                    guard.death_notifications.pop_front()
                }; // lock is dropped here — callback may now run safely

                match death_opt {
                    Some(death_box) => {
                        // Invoke callback with NO lock held
                        let on_death = death_box.on_death;
                        on_death();
                        // Box drops here (next/prev already cleaned in pop_front)
                    }
                    None => break,
                }
            }
        }

        pub fn active_node_count(&self) -> usize {
            self.active_nodes.lock().unwrap().len()
        }
    }

    // -------------------------------------------------------------------------
    // Test demonstrating safe unregister + release
    // -------------------------------------------------------------------------
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn demo_fixed_binder_node_with_unregister_and_release() {
            let manager = BinderNodeManager::new();
            let node = manager.create_node(42);

            manager
                .register_death_notification(42, 1, || println!("Recipient 1 notified"))
                .unwrap();
            manager
                .register_death_notification(42, 2, || println!("Recipient 2 notified"))
                .unwrap();

            // Unregister one while node is still alive — uses the unsafe remove path
            assert!(manager.unregister_death_notification(42, 1).is_ok());

            // Release the node — uses the fixed per-notification pop + lock drop/reacquire
            manager.release_node(42);

            // Only the remaining notification fires
            assert_eq!(manager.active_node_count(), 0);
        }
    }
}