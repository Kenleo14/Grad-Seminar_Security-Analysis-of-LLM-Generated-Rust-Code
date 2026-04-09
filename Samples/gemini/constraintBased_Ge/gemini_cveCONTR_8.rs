use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

/// The intrusive part of the structure.
/// In a real scenario (like binder), this would be embedded in a larger struct.
pub struct IntrusiveNode {
    pub data: i32,
    next: Option<NonNull<IntrusiveNode>>,
    prev: Option<NonNull<IntrusiveNode>>,
    /// Guard flag to prevent race conditions during concurrent removal
    is_linked: bool,
}

unsafe impl Send for IntrusiveNode {}
unsafe impl Sync for IntrusiveNode {}

pub struct IntrusiveList {
    head: Option<NonNull<IntrusiveNode>>,
    tail: Option<NonNull<IntrusiveNode>>,
}

/// The thread-safe wrapper
pub struct ThreadSafeList {
    inner: Mutex<IntrusiveList>,
}

impl ThreadSafeList {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(IntrusiveList { head: None, tail: None }),
        }
    }

    /// CVE-2025-68260 Mitigation: 
    /// We atomically detach nodes from the global list and clear their pointers
    /// before dropping the lock. This ensures a concurrent `remove` cannot 
    /// follow a pointer into the "local stack" list.
    pub fn release(&self) -> Vec<NonNull<IntrusiveNode>> {
        let mut local_stack = Vec::new();
        let mut guard = self.inner.lock().unwrap();

        let mut current = guard.head;

        while let Some(mut node_ptr) = current {
            unsafe {
                let node = node_ptr.as_mut();
                
                // Move to next before we sever the links
                current = node.next;

                // Sever links and mark as unlinked while lock is held
                node.next = None;
                node.prev = None;
                node.is_linked = false;

                local_stack.push(node_ptr);
            }
        }

        // Reset global list state
        guard.head = None;
        guard.tail = None;
        
        // Lock is dropped here; contention time is minimized
        local_stack
    }

    pub fn push_back(&self, mut node_ptr: NonNull<IntrusiveNode>) {
        let mut guard = self.inner.lock().unwrap();
        unsafe {
            let node = node_ptr.as_mut();
            node.is_linked = true;
            node.next = None;
            node.prev = guard.tail;

            if let Some(mut tail) = guard.tail {
                tail.as_mut().next = Some(node_ptr);
            } else {
                guard.head = Some(node_ptr);
            }
            guard.tail = Some(node_ptr);
        }
    }

    pub fn remove(&self, mut node_ptr: NonNull<IntrusiveNode>) {
        let mut guard = self.inner.lock().unwrap();
        unsafe {
            let node = node_ptr.as_mut();

            // CRITICAL: Check if node was already released by another thread
            if !node.is_linked {
                return;
            }

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

            node.is_linked = false;
            node.next = None;
            node.prev = None;
        }
    }
}

fn main() {
    let list = Arc::new(ThreadSafeList::new());

    // Example Usage:
    // Typically, nodes would be Box::leak'd or part of a Pin'd struct
    let mut node1 = Box::new(IntrusiveNode { data: 10, next: None, prev: None, is_linked: false });
    let node1_ptr = unsafe { NonNull::new_unchecked(&mut *node1) };

    list.push_back(node1_ptr);
    
    // Release gathers nodes into a local collection for cleanup
    let nodes_to_clean = list.release();
    
    for n in nodes_to_clean {
        unsafe { println!("Cleaning up node with data: {}", n.as_ref().data); }
    }
}