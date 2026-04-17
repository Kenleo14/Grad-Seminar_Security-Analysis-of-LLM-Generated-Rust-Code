use std::sync::{Arc, Mutex};
use std::ptr;

// Define a Node in the intrusive list
struct Node<T> {
    data: T,
    next: Option<*mut Node<T>>,
    prev: Option<*mut Node<T>>,
}

// Define the List
struct IntrusiveList<T> {
    head: Option<*mut Node<T>>,
    tail: Option<*mut Node<T>>,
}

impl<T> IntrusiveList<T> {
    fn new() -> Self {
        IntrusiveList {
            head: None,
            tail: None,
        }
    }

    // Safely remove a node from the list
    fn remove(&mut self, node: *mut Node<T>) {
        unsafe {
            if let Some(prev) = (*node).prev {
                (*prev).next = (*node).next;
            } else {
                self.head = (*node).next;
            }
            if let Some(next) = (*node).next {
                (*next).prev = (*node).prev;
            } else {
                self.tail = (*node).prev;
            }
            (*node).next = None;
            (*node).prev = None;
        }
    }
}

// Define a Stack
struct Stack<T> {
    data: Vec<T>,
}

impl<T> Stack<T> {
    fn new() -> Self {
        Stack { data: Vec::new() }
    }

    fn push(&mut self, value: T) {
        self.data.push(value);
    }
}

// Function to move a node from a list to a stack
fn move_node_to_stack<T>(
    list: Arc<Mutex<IntrusiveList<T>>>,
    stack: Arc<Mutex<Stack<T>>>,
    node: *mut Node<T>,
) {
    let mut list_guard = list.lock().unwrap();
    list_guard.remove(node);

    // SAFETY: This is safe because we have exclusive access to the node after removing it from the list.
    let node_data = unsafe {
        ptr::read(node).data
    };

    drop(list_guard); // Explicitly drop the lock before pushing to stack to minimize locked section

    let mut stack_guard = stack.lock().unwrap();
    stack_guard.push(node_data);
}

fn main() {
    let list = Arc::new(Mutex::new(IntrusiveList::new()));
    let stack = Arc::new(Mutex::new(Stack::new()));

    // Example usage: Add nodes to the list, and then move them to the stack
    // This part is left for implementation as per your application logic
}