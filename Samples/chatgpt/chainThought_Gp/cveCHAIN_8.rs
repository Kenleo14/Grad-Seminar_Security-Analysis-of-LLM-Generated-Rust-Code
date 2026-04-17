use std::sync::{Arc, Mutex};
use std::collections::LinkedList;

// Define a Node struct.
struct Node {
    value: i32,
    // Links for the intrusive list could be part of the node itself.
}

// Define a safe wrapper for the intrusive list using Mutex for synchronization.
struct SafeList {
    list: Mutex<LinkedList<Arc<Node>>>,
}

impl SafeList {
    fn new() -> Self {
        SafeList {
            list: Mutex::new(LinkedList::new()),
        }
    }

    // Add a node to the list
    fn add(&self, node: Arc<Node>) {
        let mut list = self.list.lock().unwrap();
        list.push_back(node);
    }

    // Safely remove a node from the list and return it
    fn remove(&self) -> Option<Arc<Node>> {
        let mut list = self.list.lock().unwrap();
        list.pop_front()
    }
}

fn main() {
    // Create a new SafeList
    let safe_list = Arc::new(SafeList::new());

    // Add nodes to the list
    for i in 0..5 {
        let node = Arc::new(Node { value: i });
        safe_list.add(node);
    }

    // Transfer nodes from the list to a stack (or another structure)
    let stack = Arc::new(Mutex::new(Vec::new()));
    let list_clone = Arc::clone(&safe_list);
    let stack_clone = Arc::clone(&stack);

    // Simulate concurrent transfer operation
    let handle = std::thread::spawn(move || {
        while let Some(node) = list_clone.remove() {
            // Safely lock the stack and push the node onto it
            let mut stack = stack_clone.lock().unwrap();
            stack.push(node);
        }
    });

    handle.join().unwrap();

    // Print the values transferred to the stack
    let stack = stack.lock().unwrap();
    for node in stack.iter() {
        println!("Node value: {}", node.value);
    }
}