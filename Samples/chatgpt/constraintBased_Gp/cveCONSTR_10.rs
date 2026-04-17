use std::ptr;
use std::sync::{Arc, Mutex};
use std::cell::UnsafeCell;

struct Node<T> {
    value: T,
    next: Option<*mut Node<T>>,
}

impl<T> Node<T> {
    fn new(value: T) -> Self {
        Node { value, next: None }
    }
}

struct IntrusiveList<T> {
    head: Option<*mut Node<T>>,
    lock: Mutex<()>,
}

impl<T> IntrusiveList<T> {
    fn new() -> Self {
        IntrusiveList { head: None, lock: Mutex::new(()) }
    }

    /// Add a node to the front of the list
    unsafe fn push_front(&mut self, node: *mut Node<T>) {
        let _guard = self.lock.lock().unwrap();
        (*node).next = self.head;
        self.head = Some(node);
    }

    /// Remove a node from the list
    unsafe fn remove(&mut self, node: *mut Node<T>) {
        let _guard = self.lock.lock().unwrap();
        let mut current = self.head;

        while let Some(current_node) = current {
            if current_node == node {
                self.head = (*current_node).next;
                return;
            }
            current = (*current_node).next;
        }
    }

    /// Release all nodes and move them to a local stack
    fn release(&mut self) -> Vec<*mut Node<T>> {
        let _guard = self.lock.lock().unwrap();
        let mut stack = Vec::new();
        let mut current = self.head;

        while let Some(node) = current {
            unsafe {
                stack.push(node);
                current = (*node).next;
            }
        }

        self.head = None;
        stack
    }
}

fn main() {
    // Example usage
    let list = Arc::new(Mutex::new(IntrusiveList::new()));

    let node1 = Box::into_raw(Box::new(Node::new(1)));
    let node2 = Box::into_raw(Box::new(Node::new(2)));

    {
        let mut list = list.lock().unwrap();
        unsafe {
            list.push_front(node1);
            list.push_front(node2);
        }
    }

    {
        let mut list = list.lock().unwrap();
        let released_nodes = list.release();
        for node in released_nodes {
            unsafe {
                println!("Released node with value: {}", (*node).value);
                Box::from_raw(node); // Clean up memory
            }
        }
    }
}