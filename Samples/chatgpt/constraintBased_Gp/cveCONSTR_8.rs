use std::sync::{Arc, Mutex};
use std::ptr::NonNull;

// Define a node in the intrusive linked list
struct Node<T> {
    value: T,
    next: Option<NonNull<Node<T>>>,
    prev: Option<NonNull<Node<T>>>,
}

struct IntrusiveList<T> {
    head: Option<NonNull<Node<T>>>,
    tail: Option<NonNull<Node<T>>>,
    lock: Mutex<()>, // Mutex to ensure thread safety
}

impl<T> IntrusiveList<T> {
    fn new() -> Self {
        Self {
            head: None,
            tail: None,
            lock: Mutex::new(()),
        }
    }

    // Add a node to the front of the list
    fn push_front(&mut self, node: NonNull<Node<T>>) {
        unsafe {
            let _guard = self.lock.lock().unwrap();
            (*node.as_ptr()).next = self.head;
            if let Some(mut head) = self.head {
                (*head.as_ptr()).prev = Some(node);
            }
            self.head = Some(node);
            if self.tail.is_none() {
                self.tail = Some(node);
            }
        }
    }

    // Remove a node from the list
    unsafe fn remove(&mut self, node: NonNull<Node<T>>) {
        let _guard = self.lock.lock().unwrap();
        let node_ref = &mut *node.as_ptr();
        if let Some(mut prev) = node_ref.prev {
            (*prev.as_ptr()).next = node_ref.next;
        } else {
            self.head = node_ref.next;
        }
        if let Some(mut next) = node_ref.next {
            (*next.as_ptr()).prev = node_ref.prev;
        } else {
            self.tail = node_ref.prev;
        }
        node_ref.next = None;
        node_ref.prev = None;
    }

    // Move all items to a local stack list
    fn release(&mut self) -> Vec<NonNull<Node<T>>> {
        let _guard = self.lock.lock().unwrap();
        let mut stack = Vec::new();
        let mut current = self.head;
        while let Some(node) = current {
            unsafe {
                let next = (*node.as_ptr()).next;
                (*node.as_ptr()).next = None; // Disconnect from the list
                (*node.as_ptr()).prev = None;
                stack.push(node);
                current = next;
            }
        }
        self.head = None;
        self.tail = None;
        stack
    }
}

// Example usage
fn main() {
    let mut list: IntrusiveList<i32> = IntrusiveList::new();

    // Create some nodes
    let node1 = Box::leak(Box::new(Node {
        value: 1,
        next: None,
        prev: None,
    }));
    let node2 = Box::leak(Box::new(Node {
        value: 2,
        next: None,
        prev: None,
    }));

    // Push nodes into the list
    unsafe {
        list.push_front(NonNull::new(node1).unwrap());
        list.push_front(NonNull::new(node2).unwrap());
    }

    // Release the list
    let nodes = list.release();
    for node in nodes {
        unsafe {
            println!("Released node with value: {}", (*node.as_ptr()).value);
        }
    }
}