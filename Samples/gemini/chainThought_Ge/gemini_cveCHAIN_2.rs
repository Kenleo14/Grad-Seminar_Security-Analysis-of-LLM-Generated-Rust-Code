impl Node {
    pub fn release(self: Arc<Self>, binder_data: &BinderData) {
        // 1. Scope the lock to the entire unlinking process
        let mut guard = binder_data.lock.lock();
        
        // 2. Check if the node is still in the list
        if self.is_linked() {
            // SAFETY: We hold the lock, ensuring no other thread can 
            // access the list. We unlink the node entirely before 
            // the lock is dropped.
            unsafe {
                // This removes the node from the intrusive list 
                // while the synchronization primitive (guard) is active.
                self.list_links.remove(&mut guard);
            }
        }
        
        // 3. The lock is dropped here (end of scope/explicit drop)
        drop(guard);

        // 4. Now it is safe to drop the Arc or move memory, 
        // as the node is no longer reachable by other threads.
        // No aliasing violation can occur.
    }
}