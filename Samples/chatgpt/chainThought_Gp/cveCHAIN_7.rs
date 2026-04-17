use std::sync::{Arc, Mutex};
use std::collections::LinkedList;

// Define a node structure for the intrusive list
struct Node {
    data: i32,
    // Other fields can be added here as needed
}

struct IntrusiveList {
    list: Mutex<LinkedList<Arc<Node>>>,  // Use Arc to allow shared ownership
}

impl IntrusiveList {
    fn new() -> Self {
        IntrusiveList {
            list: Mutex::new(LinkedList::new()),  // Initialize with an empty list
        }
    }

    // Safely add a node to the list
    fn add_node(&self, node: Arc<Node>) {
        let mut list = self.list.lock().unwrap();
        list.push_back(node);
    }

    // Safely remove a node from the list
    fn remove_node(&self, target: &Arc<Node>) -> Option<Arc<Node>> {
        let mut list = self.list.lock().unwrap();
        let position = list.iter().position(|node| Arc::ptr_eq(node, target));
        position.map(|index| list.remove(index).unwrap())
    }

    // Transfer a node to another data structure (e.g., a stack) safely
    fn move_node_to_stack(&self, target: &Arc<Node>, stack: &Mutex<Vec<Arc<Node>>>) {
        if let Some(node) = self.remove_node(target) {
            let mut stack = stack.lock().unwrap();
            stack.push(node);
        }
    }
}

fn main() {
    let list = Arc::new(IntrusiveList::new());
    let stack = Arc::new(Mutex::new(Vec::new()));

    // Create a node and add it to the list
    let node = Arc::new(Node { data: 42 });
    list.add_node(node.clone());

    // Move the node to the stack safely
    list.move_node_to_stack(&node, &stack);

    // Verify the transfer
    println!("Node transferred to stack with data: {}", stack.lock().unwrap()[0].data);
}