use std::ptr;
use std::sync::{Arc, Mutex};

struct Node {
    data: i32,
    next: *mut Node,
    prev: *mut Node,
}

impl Node {
    fn new(data: i32) -> Self {
        Node {
            data,
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }
}

struct IntrusiveList {
    head: *mut Node,
    tail: *mut Node,
    lock: Mutex<()>,
}

impl IntrusiveList {
    fn new() -> Self {
        IntrusiveList {
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
            lock: Mutex::new(()),
        }
    }

    unsafe fn add(&mut self, node: *mut Node) {
        let _guard = self.lock.lock().unwrap();

        (*node).next = self.head;
        if !self.head.is_null() {
            (*self.head).prev = node;
        }
        self.head = node;
        if self.tail.is_null() {
            self.tail = node;
        }
    }

    unsafe fn remove(&mut self, node: *mut Node) {
        let _guard = self.lock.lock().unwrap();

        if !node.is_null() {
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
        }
    }

    unsafe fn release(&mut self) -> Vec<*mut Node> {
        let _guard = self.lock.lock().unwrap();

        let mut stack = Vec::new();
        let mut current = self.head;

        while !current.is_null() {
            stack.push(current);
            current = (*current).next;
        }

        self.head = ptr::null_mut();
        self.tail = ptr::null_mut();

        stack
    }
}

fn main() {
    // Simulate usage
    let list = Arc::new(Mutex::new(IntrusiveList::new()));

    // Adding nodes
    let node1 = Box::into_raw(Box::new(Node::new(1)));
    let node2 = Box::into_raw(Box::new(Node::new(2)));
    let node3 = Box::into_raw(Box::new(Node::new(3)));

    {
        let mut list = list.lock().unwrap();
        unsafe {
            list.add(node1);
            list.add(node2);
            list.add(node3);
        }
    }

    // Releasing nodes
    let released_nodes = {
        let mut list = list.lock().unwrap();
        unsafe { list.release() }
    };

    // Cleanup released nodes
    for node in released_nodes {
        unsafe { Box::from_raw(node) }; // Convert back to Box to free memory
    }
}