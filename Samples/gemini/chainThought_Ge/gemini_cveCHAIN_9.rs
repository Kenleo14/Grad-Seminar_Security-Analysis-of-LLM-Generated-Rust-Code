// Simplified fix logic
impl Node {
    pub fn release(self: Arc<Self>, binder_state: &BinderState) {
        let mut guard = binder_state.lock();
        
        // 1. Check if the node is still in the list while HOLDING the lock.
        // We use an intrusive-safe removal that doesn't drop the lock.
        if self.links.is_linked() {
            // SAFETY: We hold the lock that protects the intrusive list,
            // ensuring no other thread can concurrently access these links.
            unsafe {
                self.links.remove_from_list();
            }
        }
        
        // 2. The node is now unlinked. Even if we drop the guard now,
        // no other thread can find the node via the global list.
        drop(guard);

        // 3. Final cleanup of the Node object can now happen safely
        // as this thread owns the last remaining reference.
    }
}