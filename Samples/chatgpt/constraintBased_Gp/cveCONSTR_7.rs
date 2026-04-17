use std::sync::{Arc, Mutex};
use std::ptr;

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

    unsafe fn insert(&mut self, node: *mut Node<T>) {
        let _guard = self.lock.lock().unwrap();
        (*node).next = self.head;
        (*node).prev = ptr::null_mut();

        if !self.head.is_null() {
            (*self.head).prev = node;
        }

        self.head = node;

        if self.tail.is_null() {
            self.tail = node;
        }
    }

    unsafe fn remove(&mut self, node: *mut Node<T>) {
        let _guard = self.lock.lock().unwrap();
        let prev = (*node).prev;
        let next = (*node).next;

        if !prev.is_null() {
            (*prev).next = next;
        } else {
            self.head = next;
        }

        if !next.is_null() {
            (*next).prev = prev;
        } else {
            self.tail = prev;
        }
    }

    fn release(&mut self) -> Vec<*mut Node<T>> {
        let _guard = self.lock.lock().unwrap();
        let mut local_stack = Vec::new();
        let mut current = self.head;

        while !current.is_null() {
            unsafe {
                let next = (*current).next;
                (*current).next = ptr::null_mut(); // Disconnect the node
                (*current).prev = ptr::null_mut();
                local_stack.push(current);
                current = next;
            }
        }

        self.head = ptr::null_mut();
        self.tail = ptr::null_mut();

        local_stack
    }
}

fn main() {
    let list = Arc::new(Mutex::new(IntrusiveList::<i32>::new()));

    // Example usage
    let node1 = Box::into_raw(Box::new(Node {
        value: 1,
        next: ptr::null_mut(),
        prev: ptr::null_mut(),
    }));

    {
        let mut list = list.lock().unwrap();
        unsafe { list.insert(node1) };
    }

    {
        let mut list = list.lock().unwrap();
        let released_nodes = list.release();
        for node in released_nodes {
            unsafe {
                println!("Released node with value: {}", (*node).value);
                Box::from_raw(node); // Reclaim memory
            }
        }
    }
}