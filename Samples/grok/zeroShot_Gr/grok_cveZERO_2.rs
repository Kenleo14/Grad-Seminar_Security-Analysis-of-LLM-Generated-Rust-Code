use std::cell::UnsafeCell;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread_local;

#[repr(C)]
pub struct DeathRecipient {
    pub prev: *mut DeathRecipient,
    pub next: *mut DeathRecipient,
    pub cookie: usize,
}

#[repr(C)]
pub struct Node {
    pub refcount: AtomicUsize,
    pub dead_next: AtomicPtr<Node>,
    pub death_mutex: Mutex<()>,
    pub death_head: UnsafeCell<*mut DeathRecipient>,
}

pub unsafe fn new_node() -> *mut Node {
    Box::into_raw(Box::new(Node::new()))
}

pub unsafe fn delete_node(node: *mut Node) {
    drop(Box::from_raw(node))
}

pub unsafe fn new_death_recipient(cookie: usize) -> *mut DeathRecipient {
    Box::into_raw(Box::new(DeathRecipient {
        prev: ptr::null_mut(),
        next: ptr::null_mut(),
        cookie,
    }))
}

pub unsafe fn delete_death_recipient(recip: *mut DeathRecipient) {
    drop(Box::from_raw(recip))
}

impl Node {
    fn new() -> Node {
        Node {
            refcount: AtomicUsize::new(1),
            dead_next: AtomicPtr::new(ptr::null_mut()),
            death_mutex: Mutex::new(()),
            death_head: UnsafeCell::new(ptr::null_mut()),
        }
    }
}

thread_local! {
    static LOCAL_DEAD_HEAD: AtomicPtr<Node> = AtomicPtr::new(ptr::null_mut());
}

/// Attempts to increment the reference count on the node (strong acquire semantics).
/// Returns `true` if successful (node was acquired), `false` if refcount was already 0.
pub unsafe fn inc_ref(node: *mut Node) -> bool {
    let mut cnt = (*node).refcount.load(Ordering::Acquire);
    while cnt != 0 {
        match (*node).refcount.compare_exchange(
            cnt,
            cnt + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(new_cnt) => cnt = new_cnt,
        }
    }
    false
}

/// Decrements the reference count. If this drops the refcount to 0, schedules the node
/// for deferred cleanup via the thread-local intrusive stack.
pub unsafe fn dec_ref(node: *mut Node) {
    if (*node).refcount.fetch_sub(1, Ordering::AcqRel) == 1 {
        push_to_local_dead(node);
    }
}

/// Adds a death recipient to the node's intrusive list. Must hold a reference (call inc_ref first externally).
/// Returns true if added successfully.
pub unsafe fn add_death_recipient(node: *mut Node, recip: *mut DeathRecipient) -> bool {
    if !inc_ref(node) {
        return false;
    }
    let guard = (*node).death_mutex.lock().unwrap();
    // Push front for simplicity.
    let head_ptr = (*node).death_head.get();
    let old_head = *head_ptr;
    (*recip).next = old_head;
    (*recip).prev = ptr::null_mut();
    if !old_head.is_null() {
        (*old_head).prev = recip;
    }
    *head_ptr = recip;
    // No need to dec_ref here; caller manages refs around this call.
    drop(guard);
    true
}

/// Removes a death recipient from the node's intrusive list by pointer.
/// Returns true if found and removed.
pub unsafe fn remove_death_recipient(node: *mut Node, recip: *mut DeathRecipient) -> bool {
    if !inc_ref(node) {
        return false;
    }
    let guard = (*node).death_mutex.lock().unwrap();
    let head_ptr = (*node).death_head.get();
    let mut cur = *head_ptr;
    while !cur.is_null() {
        if cur == recip {
            // Unlink.
            let prev = (*cur).prev;
            let next = (*cur).next;
            if prev.is_null() {
                // Was head.
                *head_ptr = next;
            } else {
                (*prev).next = next;
            }
            if !next.is_null() {
                (*next).prev = prev;
            }
            drop(guard);
            return true;
        }
        cur = (*cur).next;
    }
    drop(guard);
    false
}

/// Pushes the node onto the thread-local dead list (lock-free intrusive singly-linked stack).
unsafe fn push_to_local_dead(node: *mut Node) {
    let node_ptr = node;
    loop {
        let head = LOCAL_DEAD_HEAD.with(|h| h.load(Ordering::Acquire));
        (*node_ptr).dead_next.store(head, Ordering::Release);
        if LOCAL_DEAD_HEAD.with(|h| {
            h.compare_exchange(
                head,
                node_ptr,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
        }).is_ok() {
            break;
        }
    }
}

/// Pops a node from the thread-local dead list (lock-free).
fn pop_local_dead() -> Option<*mut Node> {
    LOCAL_DEAD_HEAD.with(|head_cell| {
        let head = head_cell;
        loop {
            let cur = head.load(Ordering::Acquire);
            if cur.is_null() {
                return None;
            }
            let next = unsafe { (*cur).dead_next.load(Ordering::Acquire) };
            if head
                .compare_exchange(
                    cur,
                    next,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                unsafe {
                    (*cur).dead_next.store(ptr::null_mut(), Ordering::Release);
                }
                return Some(cur);
            }
        }
    })
}

/// Performs cleanup on a dead node: notifies all death recipients and deletes the node.
/// Death recipients are also deleted (assumes they were allocated via new_death_recipient).
unsafe fn do_cleanup(node: *mut Node) {
    let node_ref = &mut *node;
    let guard = node_ref.death_mutex.lock().unwrap();
    let head_ptr = node_ref.death_head.get();
    let mut recip = *head_ptr;
    while !recip.is_null() {
        // Simulate death notification (in real impl, call recipient's callback).
        // e.g., println!("Death notification: node={:?}, cookie={}", node, (*recip).cookie);
        let next_recip = (*recip).next;
        delete_death_recipient(recip);
        recip = next_recip;
    }
    *head_ptr = ptr::null_mut();
    drop(guard);
    delete_node(node);
}

/// Drains the thread-local dead list and cleans up all nodes.
/// Call periodically or at thread exit for memory stability.
pub fn drain_and_cleanup() {
    while let Some(node) = pop_local_dead() {
        unsafe { do_cleanup(node) };
    }
}
