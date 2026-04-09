impl Node {
    /// Fixes CVE-2024-9174 by ensuring the lock is held 
    /// until the node is fully detached from the intrusive list.
    pub fn release(&self) {
        let mut inner = self.inner.lock();
        
        // CRITICAL: We must not drop 'inner' until the 
        // node is removed from all reachable lists.
        if let Some(links) = inner.links.take() {
            // Safety: The lock 'inner' ensures we have exclusive 
            // access to the pointers. No other thread can 
            // create an aliasing &mut T during this window.
            unsafe {
                links.remove_from_list();
            }
        }
        
        // The lock is dropped here automatically at the end of scope.
        // Only now is it safe for the memory to be repurposed 
        // or moved to a different stack/thread.
    }
}