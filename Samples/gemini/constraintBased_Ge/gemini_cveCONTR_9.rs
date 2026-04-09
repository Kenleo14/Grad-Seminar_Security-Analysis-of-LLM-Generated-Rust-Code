use std::ptr::{self, NonNull};
use std::sync::Mutex;

/// A simple intrusive node. 
/// In a real scenario, this would be a field inside a larger struct.
pub struct IntrusiveNode {
    pub next: Option<NonNull<IntrusiveNode>>,
    pub prev: Option<NonNull<IntrusiveNode>>,
}

pub struct IntrusiveList {
    head: Option<NonNull<IntrusiveNode>>,
    tail: Option<NonNull<IntrusiveNode>>,
}

impl IntrusiveList {
    pub const fn new() -> Self {
        Self { head: None, tail: None }
    }
}

/// The Container that protects the intrusive list.
pub struct SharedList {
    pub list: Mutex<IntrusiveList>,
}

impl SharedList {
    /// CVE-2025-68260 Avoidance Strategy:
    /// We lock the list, extract the entire chain to a local stack, 
    /// and reset the shared list to empty. This ensures that any concurrent 
    /// `remove()` calls will either fail to find the node or will block 
    /// until the "handover" is complete.
    pub fn release(&self) -> Vec<NonNull<IntrusiveNode>> {
        let mut guard = self.list.lock().unwrap();
        
        let mut current = guard.head;
        let mut collected = Vec::new();

        // 1. Move the items to a local collection while under the lock.
        // This minimizes the time the Mutex is held.
        while let Some(node_ptr) = current {
            unsafe {
                collected.push(node_ptr);
                // Advance to the next node before we "detach"
                current = (*node_ptr.as_ptr()).next;
                
                // CRITICAL: Clear pointers to prevent a concurrent remove() 
                // from attempting to use these pointers once we drop the lock.
                (*node_ptr.as_ptr()).next = None;
                (*node_ptr.as_ptr()).prev = None;
            }
        }

        // 2. Reset the shared list state
        guard.head = None;
        guard.tail = None;

        // Mutex drops here.
        collected
    }

    /// Thread-safe removal of a specific node.
    pub fn remove(&self, mut node_ptr: NonNull<IntrusiveNode>) {
        let mut guard = self.list.lock().unwrap();
        
        unsafe {
            let node = node_ptr.as_mut();
            
            // If the node has already been detached by release(), 
            // its pointers will be None.
            let prev = node.prev;
            let next = node.next;

            if let Some(mut p) = prev {
                p.as_mut().next = next;
            } else {
                guard.head = next;
            }

            if let Some(mut n) = next {
                n.as_mut().prev = prev;
            } else {
                guard.tail = prev;
            }

            // Mark node as detached
            node.next = None;
            node.prev = None;
        }
    }
}