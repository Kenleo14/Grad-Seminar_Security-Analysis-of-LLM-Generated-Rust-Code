use crossbeam::queue::SegQueue;
use std::sync::{Arc, Mutex};
use intrusive_collections::{LinkedList, LinkedListLink, UnsafeRef, Adapter, intrusive_adapter};

// Define a struct for the BinderNode which will hold the death notification.
struct BinderNode {
    link: LinkedListLink,
    id: usize,
    // Other fields as necessary
}

// Implement Adapter for intrusive collections.
intrusive_adapter!(BinderNodeAdapter = UnsafeRef<BinderNode>: BinderNode { link: LinkedListLink });

impl BinderNode {
    fn new(id: usize) -> Self {
        BinderNode {
            link: LinkedListLink::new(),
            id,
        }
    }
}

struct BinderNodeManager {
    nodes: LinkedList<BinderNodeAdapter>,
    temp_list: SegQueue<UnsafeRef<BinderNode>>,
    lock: Mutex<()>,
}

impl BinderNodeManager {
    fn new() -> Self {
        BinderNodeManager {
            nodes: LinkedList::new(BinderNodeAdapter::new()),
            temp_list: SegQueue::new(),
            lock: Mutex::new(()),
        }
    }

    // Add a node to the list
    fn add_node(&self, node: Arc<BinderNode>) {
        let _guard = self.lock.lock().unwrap();
        unsafe {
            self.nodes.push_back(UnsafeRef::from_arc(node));
        }
    }

    // Remove a node and move to temporary list for cleanup
    fn remove_node(&self, id: usize) {
        let _guard = self.lock.lock().unwrap();
        let mut cursor = self.nodes.cursor_mut();
        while let Some(node) = cursor.get() {
            if node.id == id {
                cursor.remove();
                self.temp_list.push(node.clone());
                break;
            }
            cursor.move_next();
        }
    }

    // Clean up nodes in the temporary list
    fn cleanup(&self) {
        while let Some(node) = self.temp_list.pop() {
            // Perform necessary cleanup
            // In a real scenario, you might notify observers or release resources
            drop(node);
        }
    }
}

fn main() {
    let manager = Arc::new(BinderNodeManager::new());

    // Example usage in a concurrent context
    let manager_clone = manager.clone();
    let handle = std::thread::spawn(move || {
        let node = Arc::new(BinderNode::new(1));
        manager_clone.add_node(node.clone());
        manager_clone.remove_node(1);
    });

    handle.join().unwrap();

    // Cleanup after handling nodes
    manager.cleanup();
}