use std::sync::{Arc, Mutex};
use std::ptr;

struct Node<T> {
    value: T,
    next: Option<*mut Node<T>>,
}

struct IntrusiveList<T> {
    head: Option<*mut Node<T>>,
}

impl<T> IntrusiveList<T> {
    fn new() -> Self {
        IntrusiveList { head: None }
    }

    // Add a node to the front of the list
    fn push_front(&mut self, node: *mut Node<T>) {
        unsafe {
            (*node).next = self.head;
            self.head = Some(node);
        }
    }

    // Move all nodes to a stack, returning the head of the stack
    fn move_to_stack(&mut self) -> Option<*mut Node<T>> {
        let head = self.head;
        self.head = None;
        head
    }
}

struct SafeIntrusiveList<T> {
    list: Arc<Mutex<IntrusiveList<T>>>,
}

impl<T> SafeIntrusiveList<T> {
    fn new() -> Self {
        SafeIntrusiveList {
            list: Arc::new(Mutex::new(IntrusiveList::new())),
        }
    }

    fn push_front(&self, node: *mut Node<T>) {
        let mut list = self.list.lock().unwrap();
        list.push_front(node);
    }

    fn move_to_stack(&self) -> Option<*mut Node<T>> {
        let mut list = self.list.lock().unwrap();
        list.move_to_stack()
    }
}

impl<T> Drop for Node<T> {
    fn drop(&mut self) {
        // Ensure proper cleanup of the node, if necessary
    }
}

fn main() {
    let list = SafeIntrusiveList::new();

    // Example usage: Creating nodes
    let node1 = Box::into_raw(Box::new(Node { value: 1, next: None }));
    let node2 = Box::into_raw(Box::new(Node { value: 2, next: None }));

    // Push nodes to the list safely
    list.push_front(node1);
    list.push_front(node2);

    // Move nodes to a stack safely
    let stack_head = list.move_to_stack();

    // Note: Proper cleanup of nodes is omitted for brevity
}