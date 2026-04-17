use std::sync::{Mutex, Arc};
use std::ptr;
use std::marker::PhantomData;

struct Node<T> {
    data: T,
    next: *mut Node<T>,
    prev: *mut Node<T>,
}

struct IntrusiveList<T> {
    head: *mut Node<T>,
    tail: *mut Node<T>,
    _marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for IntrusiveList<T> {}
unsafe impl<T: Sync> Sync for IntrusiveList<T> {}

impl<T> IntrusiveList<T> {
    fn new() -> Self {
        IntrusiveList {
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
            _marker: PhantomData,
        }
    }

    fn push_front(&mut self, node: *mut Node<T>) {
        unsafe {
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
    }

    fn remove(&mut self, node: *mut Node<T>) {
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
        }
    }
}

struct SafeList<T> {
    list: Arc<Mutex<IntrusiveList<T>>>,
}

impl<T> SafeList<T> {
    fn new() -> Self {
        SafeList {
            list: Arc::new(Mutex::new(IntrusiveList::new())),
        }
    }

    fn push_front(&self, node: *mut Node<T>) {
        let mut list = self.list.lock().unwrap();
        list.push_front(node);
    }

    fn remove(&self, node: *mut Node<T>) {
        let mut list = self.list.lock().unwrap();
        list.remove(node);
    }

    fn release(&self) -> Vec<*mut Node<T>> {
        let mut stack = Vec::new();
        let mut list = self.list.lock().unwrap();

        while !list.head.is_null() {
            unsafe {
                let node = list.head;
                list.head = (*node).next;
                (*node).prev = ptr::null_mut();
                (*node).next = ptr::null_mut();
                stack.push(node);
            }
        }
        list.tail = ptr::null_mut();

        stack
    }
}

fn main() {
    // Example usage
    let safe_list = SafeList::new();

    let node1 = Box::into_raw(Box::new(Node {
        data: 1,
        next: ptr::null_mut(),
        prev: ptr::null_mut(),
    }));

    let node2 = Box::into_raw(Box::new(Node {
        data: 2,
        next: ptr::null_mut(),
        prev: ptr::null_mut(),
    }));

    safe_list.push_front(node1);
    safe_list.push_front(node2);

    // Release the list into a local stack
    let stack = safe_list.release();
    for node in stack {
        unsafe {
            println!("Node data: {}", (*node).data);
            Box::from_raw(node); // Clean up
        }
    }
}