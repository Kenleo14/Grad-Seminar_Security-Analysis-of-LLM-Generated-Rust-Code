use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

/// The Intrusive Node that must be embedded in your data structure.
#[repr(C)]
pub struct IntrusiveNode {
    next: Option<NonNull<IntrusiveNode>>,
    prev: Option<NonNull<IntrusiveNode>>,
}

/// The actual data containing the intrusive hook.
pub struct MyData {
    pub value: i32,
    pub list_node: IntrusiveNode,
}

/// The List Head structure managing the pointers.
pub struct IntrusiveList {
    head: Option<NonNull<IntrusiveNode>>,
    tail: Option<NonNull<IntrusiveNode>>,
}

impl IntrusiveList {
    pub const fn new() -> Self {
        Self { head: None, tail: None }
    }

    /// Safely adds a node to the front of the list.
    pub unsafe fn push_front(&mut self, mut node_ptr: NonNull<IntrusiveNode>) {
        let node = node_ptr.as_mut();
        node.next = self.head;
        node.prev = None;

        if let Some(mut old_head) = self.head {
            old_head.as_mut().prev = Some(node_ptr);
        } else {
            self.tail = Some(node_ptr);
        }
        self.head = Some(node_ptr);
    }

    /// The Cleanup Routine: Mitigates CVE-2025-68260.
    /// It "steals" the entire list into a local variable while holding the lock,
    /// then processes (cleans up) the nodes outside the lock.
    pub fn release(list_mutex: &Mutex<IntrusiveList>) {
        let mut local_head: Option<NonNull<IntrusiveNode>>;

        // --- CRITICAL SECTION START ---
        {
            let mut list = list_mutex.lock().unwrap();
            
            // Atomically detach the entire list.
            // Any concurrent remove() or push() calls after this point
            // will see an empty list, preventing pointer corruption.
            local_head = list.head;
            
            list.head = None;
            list.tail = None;
        }
        // --- CRITICAL SECTION END ---

        // Now we process the 'stolen' list on the local stack.
        // Contention is minimized because the lock is already released.
        let mut current = local_head;
        while let Some(mut node_ptr) = current {
            unsafe {
                let node = node_ptr.as_mut();
                let next_node = node.next;

                // Neutralize pointers to prevent UAF or double-removal
                node.next = None;
                node.prev = None;

                // Here you would perform the actual cleanup 
                // (e.g., dropping the container_of or freeing memory)
                println!("Cleaning up node with pointer: {:?}", node_ptr);

                current = next_node;
            }
        }
    }

    /// Thread-safe removal of a single node.
    pub unsafe fn remove(&mut self, mut node_ptr: NonNull<IntrusiveNode>) {
        let node = node_ptr.as_mut();

        // If node is already detached (e.g., by release()), do nothing.
        if node.prev.is_none() && self.head != Some(node_ptr) {
            return;
        }

        if let Some(mut prev) = node.prev {
            prev.as_mut().next = node.next;
        } else {
            self.head = node.next;
        }

        if let Some(mut next) = node.next {
            next.as_mut().prev = node.prev;
        } else {
            self.tail = node.prev;
        }

        node.next = None;
        node.prev = None;
    }
}

fn main() {
    let list = Arc::new(Mutex::new(IntrusiveList::new()));

    // Example Usage
    let mut data = MyData {
        value: 42,
        list_node: IntrusiveNode { next: None, prev: None },
    };

    unsafe {
        let node_ptr = NonNull::new_unchecked(&mut data.list_node);
        list.lock().unwrap().push_front(node_ptr);
    }

    // Trigger the thread-safe cleanup
    IntrusiveList::release(&list);
}