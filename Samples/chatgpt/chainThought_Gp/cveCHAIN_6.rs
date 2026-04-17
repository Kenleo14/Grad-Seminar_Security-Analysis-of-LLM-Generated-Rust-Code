use std::sync::{Arc, Mutex};
use std::thread;

// Define a Node in the intrusive list
struct Node {
    value: i32,
    next: Option<Arc<Mutex<Node>>>,
}

impl Node {
    fn new(value: i32) -> Self {
        Node { value, next: None }
    }
}

// Define the intrusive list
struct IntrusiveList {
    head: Option<Arc<Mutex<Node>>>,
}

impl IntrusiveList {
    fn new() -> Self {
        IntrusiveList { head: None }
    }

    // Add a node to the list
    fn push(&mut self, value: i32) {
        let new_node = Arc::new(Mutex::new(Node::new(value)));
        if let Some(head) = &self.head {
            new_node.lock().unwrap().next = Some(Arc::clone(&head));
        }
        self.head = Some(new_node);
    }

    // Safely remove a node from the list
    fn remove(&mut self, value: i32) -> Option<Node> {
        let mut current = self.head.take();
        let mut prev: Option<Arc<Mutex<Node>>> = None;

        while let Some(node) = current {
            let mut node_lock = node.lock().unwrap();
            if node_lock.value == value {
                if let Some(prev_node) = prev {
                    prev_node.lock().unwrap().next = node_lock.next.take();
                } else {
                    self.head = node_lock.next.take();
                }
                return Some(Arc::try_unwrap(node).ok().unwrap().into_inner().unwrap());
            }
            prev = Some(node);
            current = node_lock.next.clone();
        }
        None
    }

    // Safely move a node to the stack
    fn move_to_stack(&mut self, value: i32) -> Option<Node> {
        // Remove the node from the list
        self.remove(value)
    }
}

fn main() {
    let list = Arc::new(Mutex::new(IntrusiveList::new()));
    list.lock().unwrap().push(1);
    list.lock().unwrap().push(2);
    list.lock().unwrap().push(3);

    // Example of concurrent access
    let list_clone = Arc::clone(&list);
    let handle = thread::spawn(move || {
        if let Some(node) = list_clone.lock().unwrap().move_to_stack(2) {
            println!("Moved node with value: {}", node.value);
        }
    });

    if let Some(node) = list.lock().unwrap().move_to_stack(1) {
        println!("Moved node with value: {}", node.value);
    }

    handle.join().unwrap();
}