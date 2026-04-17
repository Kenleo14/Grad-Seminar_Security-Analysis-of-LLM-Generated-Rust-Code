use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::ops::{Deref, DerefMut};
use std::ptr;

// Define a structure for BinderNode
struct BinderNode {
    // Unique identifier for the BinderNode
    id: usize,
    // Atomic flag to mark if the node is alive
    alive: AtomicBool,
    // Pointer to the next node in the intrusive linked list
    next: *mut BinderNode,
}

// Implement methods for BinderNode
impl BinderNode {
    fn new(id: usize) -> Self {
        BinderNode {
            id,
            alive: AtomicBool::new(true),
            next: ptr::null_mut(),
        }
    }

    fn mark_dead(&self) {
        self.alive.store(false, Ordering::SeqCst);
    }
}

// Define a structure for managing the linked list
struct BinderNodeList {
    head: *mut BinderNode,
}

impl BinderNodeList {
    fn new() -> Self {
        BinderNodeList {
            head: ptr::null_mut(),
        }
    }

    fn add_node(&mut self, node: *mut BinderNode) {
        unsafe {
            (*node).next = self.head;
            self.head = node;
        }
    }

    fn move_dead_nodes(&mut self) -> BinderNodeList {
        let mut dead_list = BinderNodeList::new();

        let mut current = self.head;
        let mut prev = ptr::null_mut();

        while !current.is_null() {
            unsafe {
                if !(*current).alive.load(Ordering::SeqCst) {
                    if prev.is_null() {
                        self.head = (*current).next;
                    } else {
                        (*prev).next = (*current).next;
                    }

                    let next = (*current).next;
                    (*current).next = dead_list.head;
                    dead_list.head = current;
                    current = next;
                } else {
                    prev = current;
                    current = (*current).next;
                }
            }
        }

        dead_list
    }
}

// Thread-safe wrapper around BinderNodeList
struct BinderNodeManager {
    list: Arc<Mutex<BinderNodeList>>,
}

impl BinderNodeManager {
    fn new() -> Self {
        BinderNodeManager {
            list: Arc::new(Mutex::new(BinderNodeList::new())),
        }
    }

    fn add_node(&self, node: Arc<BinderNode>) {
        let node_ptr = Arc::into_raw(node) as *mut _;
        let mut list = self.list.lock().unwrap();
        list.add_node(node_ptr);
    }

    fn cleanup_dead_nodes(&self) {
        let mut list = self.list.lock().unwrap();
        let dead_list = list.move_dead_nodes();

        // Free the dead nodes
        let mut current = dead_list.head;
        while !current.is_null() {
            unsafe {
                let next = (*current).next;
                let _ = Arc::from_raw(current); // Reclaim the Arc and drop it
                current = next;
            }
        }
    }
}

fn main() {
    let manager = BinderNodeManager::new();

    // Simulate adding nodes
    for id in 0..10 {
        let node = Arc::new(BinderNode::new(id));
        manager.add_node(node.clone());

        if id % 2 == 0 {
            node.mark_dead();
        }
    }

    // Simulate concurrent cleanup
    manager.cleanup_dead_nodes();
}