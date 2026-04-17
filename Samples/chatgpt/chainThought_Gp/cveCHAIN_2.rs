use std::sync::{Arc, Mutex};
use std::collections::LinkedList;

// Define a struct for list nodes
struct Node {
    value: i32,
    // Additional fields for an intrusive list could be here
}

// Define a thread-safe wrapper around our intrusive list
struct SafeList {
    list: Mutex<LinkedList<Node>>,
}

impl SafeList {
    fn new() -> Self {
        SafeList {
            list: Mutex::new(LinkedList::new()),
        }
    }

    // Safely add a node to the list
    fn add_node(&self, value: i32) {
        let mut list = self.list.lock().unwrap();
        list.push_back(Node { value });
    }

    // Safely remove a node from the list and return it
    fn remove_node(&self) -> Option<Node> {
        let mut list = self.list.lock().unwrap();
        list.pop_front()
    }
}

// Define a thread-safe stack for nodes
struct SafeStack {
    stack: Mutex<Vec<Node>>,
}

impl SafeStack {
    fn new() -> Self {
        SafeStack {
            stack: Mutex::new(Vec::new()),
        }
    }

    // Push a node onto the stack
    fn push_node(&self, node: Node) {
        let mut stack = self.stack.lock().unwrap();
        stack.push(node);
    }

    // Pop a node from the stack
    fn pop_node(&self) -> Option<Node> {
        let mut stack = self.stack.lock().unwrap();
        stack.pop()
    }
}

// Example function to transfer nodes from list to stack
fn transfer_nodes(list: Arc<SafeList>, stack: Arc<SafeStack>) {
    while let Some(node) = list.remove_node() {
        stack.push_node(node);
    }
}

fn main() {
    let list = Arc::new(SafeList::new());
    let stack = Arc::new(SafeStack::new());

    // Add some nodes to the list
    list.add_node(1);
    list.add_node(2);
    list.add_node(3);

    // Transfer nodes to the stack
    transfer_nodes(list.clone(), stack.clone());

    // Example usage: pop nodes from the stack
    while let Some(node) = stack.pop_node() {
        println!("Popped node with value: {}", node.value);
    }
}