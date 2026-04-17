use std::sync::{Arc, Mutex};
use std::ptr;

// Define a node structure for the intrusive list
struct Node<T> {
    data: T,
    next: Option<*mut Node<T>>,
    prev: Option<*mut Node<T>>,
}

// Define the list structure
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

    // Add a method to safely remove a node
    fn remove(&mut self, node: *mut Node<T>) {
        unsafe {
            if let Some(prev) = (*node).prev {
                (*prev).next = (*node).next;
            } else {
                // Node is head
                self.head = (*node).next;
            }

            if let Some(next) = (*node).next {
                (*next).prev = (*node).prev;
            } else {
                // Node is tail
                self.tail = (*node).prev;
            }

            (*node).next = None;
            (*node).prev = None;
        }
    }

    // Add a method to safely add a node
    fn push(&mut self, node: *mut Node<T>) {
        unsafe {
            (*node).next = self.head;
            (*node).prev = None;

            if let Some(head) = self.head {
                (*head).prev = Some(node);
            }

            self.head = Some(node);

            if self.tail.is_none() {
                self.tail = Some(node);
            }
        }
    }
}

// Define a synchronized list structure
struct SyncList<T> {
    list: Arc<Mutex<IntrusiveList<T>>>,
}

impl<T> SyncList<T> {
    fn new() -> Self {
        SyncList {
            list: Arc::new(Mutex::new(IntrusiveList::new())),
        }
    }

    fn push(&self, node: *mut Node<T>) {
        let mut list = self.list.lock().unwrap();
        list.push(node);
    }

    fn remove(&self, node: *mut Node<T>) {
        let mut list = self.list.lock().unwrap();
        list.remove(node);
    }
}

// Usage
fn main() {
    let list = SyncList::new();

    // Create some nodes
    let node1 = Box::into_raw(Box::new(Node {
        data: 1,
        next: None,
        prev: None,
    }));
    let node2 = Box::into_raw(Box::new(Node {
        data: 2,
        next: None,
        prev: None,
    }));

    // Safely push nodes
    list.push(node1);
    list.push(node2);

    // Safely remove a node
    list.remove(node1);

    // Clean up
    unsafe {
        Box::from_raw(node1);
        Box::from_raw(node2);
    }
}