use std::sync::{Arc, Mutex};
use std::collections::LinkedList;

// Define a node structure
struct Node {
    data: i32,
    // Other fields...
}

// Define a thread-safe intrusive list
struct IntrusiveList {
    nodes: Mutex<LinkedList<Arc<Mutex<Node>>>>,
}

impl IntrusiveList {
    fn new() -> Self {
        IntrusiveList {
            nodes: Mutex::new(LinkedList::new()),
        }
    }

    fn add_node(&self, node: Node) {
        let mut nodes = self.nodes.lock().unwrap();
        nodes.push_back(Arc::new(Mutex::new(node)));
    }

    fn move_to_stack(&self, stack: &mut Vec<Arc<Mutex<Node>>>) {
        let mut nodes = self.nodes.lock().unwrap();
        if let Some(node) = nodes.pop_front() {
            stack.push(node);
        }
    }
}

fn main() {
    // Create a new intrusive list and stack
    let list = Arc::new(IntrusiveList::new());
    let mut stack: Vec<Arc<Mutex<Node>>> = Vec::new();

    // Add nodes to the list
    list.add_node(Node { data: 1 });
    list.add_node(Node { data: 2 });

    // Safely move a node from the list to the stack
    list.move_to_stack(&mut stack);

    // Access the node from the stack
    if let Some(node) = stack.pop() {
        let node = node.lock().unwrap();
        println!("Moved node with data: {}", node.data);
    }
}