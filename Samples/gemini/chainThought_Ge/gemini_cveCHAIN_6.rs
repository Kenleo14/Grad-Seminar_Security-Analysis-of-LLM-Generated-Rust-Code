impl Node {
    pub fn release(self: Arc<Self>, mut guard: Guard<SpinLock>) {
        // 1. Maintain the lock while checking the state
        if self.is_in_list() {
            // 2. Perform the UNLINK while the guard is still active.
            // This ensures no other thread can find the node in the list
            // while we are preparing to move it.
            unsafe {
                self.links.remove_from_list_under_lock(&mut guard);
            }
        }
        
        // 3. NOW it is safe to drop the lock.
        drop(guard);
        
        // 4. Proceed with cleanup knowing no aliases exist in the global list.
        self.perform_final_cleanup();
    }
}