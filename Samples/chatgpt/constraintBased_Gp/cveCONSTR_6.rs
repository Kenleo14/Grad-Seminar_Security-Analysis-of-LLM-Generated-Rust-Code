use std::sync::{Arc, Mutex};
use std::ptr;
use std::mem;

struct Node<T> {
    value: T,
    next: *mut Node<T>,
    prev: *mut Node<T>,
}

struct IntrusiveList<T> {
    head: *mut Node<T>,
    tail: *mut Node<T>,
    lock: Mutex<()>,
}

impl<T> IntrusiveList<T> {
    fn new() -> Self {
        IntrusiveList {
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
            lock: Mutex::new(()),
        }
    }

    // Adds a new node to the head of the list
    fn push_front(&mut self, value: T) {
        let new_node = Box::into_raw(Box::new(Node {
            value,
            next: self.head,
            prev: ptr::null_mut(),
        }));

        unsafe {
            if !self.head.is_null() {
                (*self.head).prev = new_node;
            }
            self.head = new_node;
            if self.tail.is_null() {
                self.tail = new_node;
            }
        }
    }

    // Thread-safe release function
    fn release(&mut self) -> Vec<T> {
        let _guard = self.lock.lock().unwrap();

        let mut stack = Vec::new();
        let mut current = self.head;

        while !current.is_null() {
            unsafe {
                stack.push(ptr::read(&(*current).value));
                let next = (*current).next;
                Box::from_raw(current); // Free the node
                current = next;
            }
        }

        self.head = ptr::null_mut();
        self.tail = ptr::null_mut();

        stack
    }

    // Safe remove function
    fn remove(&mut self, node: *mut Node<T>) {
        let _guard = self.lock.lock().unwrap();

        unsafe {
            if !(*node).prev.is_null() {
                (*(*node).prev).next = (*node).next;
            } else {
                self.head = (*node).next;
            }

            if !(*node).next.is_null() {
                (*(*node).next).prev = (*node).prev;
            } else {
                self.tail = (*node).prev;
            }

            Box::from_raw(node); // Free the node
        }
    }
}

// Example usage
fn main() {
    let mut list = IntrusiveList::new();

    list.push_front(1);
    list.push_front(2);
    list.push_front(3);

    let released_items = list.release();
    println!("{:?}", released_items);
}