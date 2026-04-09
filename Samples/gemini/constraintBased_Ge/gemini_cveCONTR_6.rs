use std::sync::Mutex;
use std::ptr;

/// The intrusive link structure. 
/// In a real binder-like scenario, this would be embedded in a larger struct.
pub struct IntrusiveNode {
    pub next: *mut IntrusiveNode,
    pub prev: *mut IntrusiveNode,
    // Data would go here
}

pub struct IntrusiveList {
    head: *mut IntrusiveNode,
    tail: *mut IntrusiveNode,
    size: usize,
}

/// The wrapper that protects the list.
pub struct SharedList {
    inner: Mutex<IntrusiveList>,
}

impl SharedList {
    /// CVE-2025-68260 Safe Cleanup Routine
    /// Moves items to a local stack to minimize lock holding time.
    pub unsafe fn release_all(&self) {
        let mut local_cleanup_stack: Vec<*mut IntrusiveNode> = Vec::new();

        // 1. Acquire lock to perform the "Move"
        {
            let mut list = self.inner.lock().unwrap();
            let mut current = list.head;

            while !current.is_null() {
                let node = current;
                
                // Advance current before we mangle the pointers
                current = (*node).next;

                // CRITICAL: Fully isolate the node while under the lock.
                // This prevents a concurrent remove() from seeing 
                // partially valid pointers.
                local_cleanup_stack.push(node);

                // Reset pointers to prevent double-unlinking/UAF
                (*node).next = ptr::null_mut();
                (*node).prev = ptr::null_mut();
            }

            // Reset the main list state
            list.head = ptr::null_mut();
            list.tail = ptr::null_mut();
            list.size = 0;
        } 
        // 2. Lock is dropped here.

        // 3. Process the local stack (potentially expensive cleanup)
        for node_ptr in local_cleanup_stack {
            // Reconstruct the Box or trigger the specific release logic
            // Box::from_raw(node_ptr); 
            println!("Safely cleaned up node at {:?}", node_ptr);
        }
    }
}