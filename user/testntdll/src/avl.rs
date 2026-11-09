//! The AVL generic tables (`RtlInitializeGenericTableAvl` and its
//! companions), as the WDK documents them: a caller-owned RTL_AVL_TABLE
//! (ntddk.h's layout: the root's balanced links, the ordered-element
//! cache, the counts, the restart key and the caller's compare, allocate
//! and free routines and context), elements copied into nodes the
//! caller's allocate routine makes: an RTL_BALANCED_LINKS header (parent,
//! left, right, balance), padded to 8 bytes, then the element. Lookups
//! use the compare routine (GenericLessThan 0, GenericGreaterThan 1,
//! GenericEqual 2).
//!
//! The tree is an AVL tree threaded through the headers (each node's
//! subtree height in its first reserved byte, so a change costs the path
//! to the root); the root's links
//! (`BalancedRoot`) are a sentinel whose right child is the real root, so
//! no node's parent is null. `NumberGenericTableElements`,
//! `WhichOrderedElement`/`OrderedPointer` (the cache that
//! `RtlGetElementGenericTableAvl` walks from) and `RestartKey` (where
//! `RtlEnumerateGenericTableAvl` continues) are kept in the table, so a
//! program that reads the count from the structure sees it.

use core::ffi::c_void;
use core::ptr::null_mut;

/// RTL_BALANCED_LINKS.
#[repr(C)]
struct Links {
    parent: *mut Links,
    left: *mut Links,
    right: *mut Links,
    balance: i8,
    reserved: [u8; 3],
}

/// The bytes before an element in its node: the links, 8-aligned.
const HEADER: usize = size_of::<Links>().div_ceil(8) * 8;

type Compare = extern "system" fn(*mut AvlTable, *const c_void, *const c_void) -> u32;
type Allocate = extern "system" fn(*mut AvlTable, u32) -> *mut c_void;
type Free = extern "system" fn(*mut AvlTable, *mut c_void);

/// RTL_AVL_TABLE.
#[repr(C)]
pub struct AvlTable {
    root: Links,
    ordered_pointer: *mut c_void,
    which_ordered_element: u32,
    number_elements: u32,
    depth: u32,
    restart_key: *mut Links,
    delete_count: u32,
    compare: Option<Compare>,
    allocate: Option<Allocate>,
    free: Option<Free>,
    context: *mut c_void,
}

const _: () = assert!(size_of::<AvlTable>() == if cfg!(target_arch = "x86") { 56 } else { 104 });

const LESS: u32 = 0;
const GREATER: u32 = 1;

fn element(node: *mut Links) -> *mut c_void {
    // SAFETY: the element follows the header in the node.
    unsafe { node.cast::<u8>().add(HEADER).cast() }
}

/// The real root (the sentinel's right child), or null.
fn top(t: *mut AvlTable) -> *mut Links {
    // SAFETY: the caller's initialized table.
    unsafe { (*t).root.right }
}

fn compare(t: *mut AvlTable, a: *const c_void, b: *const c_void) -> u32 {
    // SAFETY: the caller's initialized table.
    match unsafe { (*t).compare } {
        Some(f) => f(t, a, b),
        None => 2,
    }
}

/// RtlInitializeGenericTableAvl(table, compare, allocate, free, context).
#[unsafe(no_mangle)]
pub extern "system" fn RtlInitializeGenericTableAvl(
    table: *mut AvlTable,
    compare: Compare,
    allocate: Allocate,
    free: Free,
    context: *mut c_void,
) {
    // SAFETY: the caller's table.
    unsafe {
        let sentinel = &raw mut (*table).root;
        table.write(AvlTable {
            root: Links {
                parent: sentinel,
                left: null_mut(),
                right: null_mut(),
                balance: 0,
                reserved: [0; 3],
            },
            ordered_pointer: null_mut(),
            which_ordered_element: 0,
            number_elements: 0,
            depth: 0,
            restart_key: null_mut(),
            delete_count: 0,
            compare: Some(compare),
            allocate: Some(allocate),
            free: Some(free),
            context,
        });
    }
}

/// The node equal to `key`, or (None, the parent it would hang from,
/// whether on the left).
fn find(t: *mut AvlTable, key: *const c_void) -> Result<*mut Links, (*mut Links, bool)> {
    // SAFETY: the table's nodes.
    unsafe {
        let mut parent = &raw mut (*t).root;
        let mut left = false;
        let mut n = top(t);
        while !n.is_null() {
            match compare(t, key, element(n)) {
                LESS => {
                    parent = n;
                    left = true;
                    n = (*n).left;
                }
                GREATER => {
                    parent = n;
                    left = false;
                    n = (*n).right;
                }
                _ => return Ok(n),
            }
        }
        Err((parent, left))
    }
}

/// A subtree's height, kept in the node's first reserved byte (0: none).
fn height(n: *mut Links) -> i32 {
    if n.is_null() {
        return 0;
    }
    // SAFETY: a node of the tree.
    i32::from(unsafe { (*n).reserved[0] })
}

/// Sets `n`'s height and balance from its children's.
fn update(n: *mut Links) {
    if n.is_null() {
        return;
    }
    // SAFETY: a node of the tree.
    unsafe {
        let (l, r) = (height((*n).left), height((*n).right));
        (*n).reserved[0] = (1 + l.max(r)) as u8;
        (*n).balance = (r - l) as i8;
    }
}

/// Replaces `old` by `new` in `old`'s parent.
///
/// # Safety
/// Both are nodes of the table (or `new` null); `old` has a parent.
unsafe fn replace_child(old: *mut Links, new: *mut Links) {
    // SAFETY: per the contract.
    unsafe {
        let p = (*old).parent;
        if (*p).left == old {
            (*p).left = new;
        } else {
            (*p).right = new;
        }
        if !new.is_null() {
            (*new).parent = p;
        }
    }
}

/// Rotates `n` down to the left (its right child comes up); the new top.
///
/// # Safety
/// `n` and its right child are nodes of the table.
unsafe fn rotate_left(n: *mut Links) -> *mut Links {
    // SAFETY: per the contract.
    unsafe {
        let r = (*n).right;
        replace_child(n, r);
        (*n).right = (*r).left;
        if !(*r).left.is_null() {
            (*(*r).left).parent = n;
        }
        (*r).left = n;
        (*n).parent = r;
        r
    }
}

/// Rotates `n` down to the right (its left child comes up).
///
/// # Safety
/// `n` and its left child are nodes of the table.
unsafe fn rotate_right(n: *mut Links) -> *mut Links {
    // SAFETY: per the contract.
    unsafe {
        let l = (*n).left;
        replace_child(n, l);
        (*n).left = (*l).right;
        if !(*l).right.is_null() {
            (*(*l).right).parent = n;
        }
        (*l).right = n;
        (*n).parent = l;
        l
    }
}

/// Recomputes the balances from `from` up to the sentinel, rotating where
/// a side is two taller (balance: right height minus left height).
fn rebalance(t: *mut AvlTable, from: *mut Links) {
    // SAFETY: the table's nodes; the sentinel ends the walk.
    unsafe {
        let sentinel = &raw mut (*t).root;
        let mut n = from;
        while !n.is_null() && n != sentinel {
            update(n);
            let b = height((*n).right) - height((*n).left);
            let mut top_of = n;
            if b > 1 {
                let r = (*n).right;
                if height((*r).left) > height((*r).right) {
                    rotate_right(r);
                }
                top_of = rotate_left(n);
            } else if b < -1 {
                let l = (*n).left;
                if height((*l).right) > height((*l).left) {
                    rotate_left(l);
                }
                top_of = rotate_right(n);
            }
            for m in [(*top_of).left, (*top_of).right, top_of] {
                update(m);
            }
            n = (*top_of).parent;
        }
        (*t).depth = height(top(t)) as u32;
    }
}

/// The node after `n` in order (null at the end).
fn successor(t: *mut AvlTable, n: *mut Links) -> *mut Links {
    // SAFETY: the table's nodes.
    unsafe {
        let sentinel = &raw mut (*t).root;
        if !(*n).right.is_null() {
            let mut m = (*n).right;
            while !(*m).left.is_null() {
                m = (*m).left;
            }
            return m;
        }
        let mut m = n;
        let mut p = (*m).parent;
        while p != sentinel && (*p).right == m {
            m = p;
            p = (*p).parent;
        }
        if p == sentinel { null_mut() } else { p }
    }
}

fn first(t: *mut AvlTable) -> *mut Links {
    let mut n = top(t);
    if n.is_null() {
        return n;
    }
    // SAFETY: the table's nodes.
    unsafe {
        while !(*n).left.is_null() {
            n = (*n).left;
        }
    }
    n
}

/// A change in the set: the order cache and restart key start over.
fn changed(t: *mut AvlTable) {
    // SAFETY: the caller's table.
    unsafe {
        (*t).ordered_pointer = null_mut();
        (*t).which_ordered_element = 0;
        (*t).restart_key = null_mut();
    }
}

/// RtlInsertElementGenericTableAvl(table, buffer, size, &new): the
/// element equal to `buffer` if there is one (`new` FALSE), else a copy
/// of its `size` bytes in a node from the allocate routine (`new` TRUE);
/// null if that fails.
#[unsafe(no_mangle)]
pub extern "system" fn RtlInsertElementGenericTableAvl(
    table: *mut AvlTable,
    buffer: *const c_void,
    size: u32,
    new: *mut u8,
) -> *mut c_void {
    let (parent, left) = match find(table, buffer) {
        Ok(n) => {
            if !new.is_null() {
                // SAFETY: the caller's BOOLEAN.
                unsafe { *new = 0 };
            }
            return element(n);
        }
        Err(at) => at,
    };
    // SAFETY: the caller's initialized table.
    let Some(allocate) = (unsafe { (*table).allocate }) else { return null_mut() };
    let node = allocate(table, (HEADER + size as usize) as u32).cast::<Links>();
    if node.is_null() {
        return null_mut();
    }
    // SAFETY: a fresh node of HEADER + size bytes; the parent is the
    // sentinel or a node.
    unsafe {
        node.write(Links {
            parent,
            left: null_mut(),
            right: null_mut(),
            balance: 0,
            reserved: [1, 0, 0],
        });
        core::ptr::copy_nonoverlapping(
            buffer.cast::<u8>(),
            element(node).cast::<u8>(),
            size as usize,
        );
        let sentinel = &raw mut (*table).root;
        if parent == sentinel || !left {
            (*parent).right = node;
        } else {
            (*parent).left = node;
        }
        (*table).number_elements += 1;
        if !new.is_null() {
            *new = 1;
        }
    }
    changed(table);
    rebalance(table, parent);
    element(node)
}

/// RtlDeleteElementGenericTableAvl(table, buffer): removes the element
/// equal to `buffer` (its node to the free routine); FALSE if none.
#[unsafe(no_mangle)]
pub extern "system" fn RtlDeleteElementGenericTableAvl(
    table: *mut AvlTable,
    buffer: *const c_void,
) -> u8 {
    let Ok(n) = find(table, buffer) else { return 0 };
    // SAFETY: `n` and its relatives are nodes of the table.
    let from = unsafe {
        if !(*n).left.is_null() && !(*n).right.is_null() {
            // Two children: the successor takes the node's place.
            let mut s = (*n).right;
            while !(*s).left.is_null() {
                s = (*s).left;
            }
            let from = if (*s).parent == n { s } else { (*s).parent };
            if (*s).parent != n {
                replace_child(s, (*s).right);
                (*s).right = (*n).right;
                (*(*s).right).parent = s;
            }
            replace_child(n, s);
            (*s).left = (*n).left;
            (*(*s).left).parent = s;
            from
        } else {
            let child = if (*n).left.is_null() { (*n).right } else { (*n).left };
            let parent = (*n).parent;
            replace_child(n, child);
            parent
        }
    };
    // SAFETY: the caller's initialized table.
    unsafe {
        (*table).number_elements -= 1;
        (*table).delete_count += 1;
        if let Some(free) = (*table).free {
            free(table, n.cast());
        }
    }
    changed(table);
    rebalance(table, from);
    1
}

/// RtlLookupElementGenericTableAvl(table, buffer): the element equal to
/// `buffer`, or null.
#[unsafe(no_mangle)]
pub extern "system" fn RtlLookupElementGenericTableAvl(
    table: *mut AvlTable,
    buffer: *const c_void,
) -> *mut c_void {
    match find(table, buffer) {
        Ok(n) => element(n),
        Err(_) => null_mut(),
    }
}

/// RtlEnumerateGenericTableAvl(table, restart): the first element in
/// order (`restart` TRUE), then the next on each call; null at the end.
#[unsafe(no_mangle)]
pub extern "system" fn RtlEnumerateGenericTableAvl(
    table: *mut AvlTable,
    restart: u8,
) -> *mut c_void {
    // SAFETY: the caller's initialized table.
    let key = unsafe { (*table).restart_key };
    let next = if restart != 0 || key.is_null() { first(table) } else { successor(table, key) };
    // SAFETY: as above.
    unsafe { (*table).restart_key = next };
    if next.is_null() { null_mut() } else { element(next) }
}

/// RtlEnumerateGenericTableWithoutSplayingAvl(table, &restart_key): as
/// RtlEnumerateGenericTableAvl with the place in the caller's key (null:
/// from the start).
#[unsafe(no_mangle)]
pub extern "system" fn RtlEnumerateGenericTableWithoutSplayingAvl(
    table: *mut AvlTable,
    restart_key: *mut *mut c_void,
) -> *mut c_void {
    // SAFETY: the caller's key.
    let key = unsafe { *restart_key }.cast::<Links>();
    let next = if key.is_null() { first(table) } else { successor(table, key) };
    // SAFETY: as above.
    unsafe { *restart_key = next.cast() };
    if next.is_null() { null_mut() } else { element(next) }
}

/// RtlGetElementGenericTableAvl(table, index): the element at `index` in
/// order (0 first), or null. Consecutive indexes walk on from the cached
/// one.
#[unsafe(no_mangle)]
pub extern "system" fn RtlGetElementGenericTableAvl(
    table: *mut AvlTable,
    index: u32,
) -> *mut c_void {
    // SAFETY: the caller's initialized table.
    unsafe {
        if index >= (*table).number_elements {
            return null_mut();
        }
        let cached = (*table).ordered_pointer.cast::<Links>();
        let (mut n, mut at) = if !cached.is_null() && (*table).which_ordered_element <= index {
            (cached, (*table).which_ordered_element)
        } else {
            (first(table), 0)
        };
        while at < index && !n.is_null() {
            n = successor(table, n);
            at += 1;
        }
        if n.is_null() {
            return null_mut();
        }
        (*table).ordered_pointer = n.cast();
        (*table).which_ordered_element = index;
        element(n)
    }
}

/// RtlNumberGenericTableElementsAvl(table).
#[unsafe(no_mangle)]
pub extern "system" fn RtlNumberGenericTableElementsAvl(table: *mut AvlTable) -> u32 {
    // SAFETY: the caller's initialized table.
    unsafe { (*table).number_elements }
}

/// RtlIsGenericTableEmptyAvl(table).
#[unsafe(no_mangle)]
pub extern "system" fn RtlIsGenericTableEmptyAvl(table: *mut AvlTable) -> u8 {
    u8::from(top(table).is_null())
}

/// RtlLookupFirstMatchingElementGenericTableAvl(table, buffer, &restart):
/// the element equal to `buffer` (the tree has no duplicates), its node
/// as the restart key for RtlEnumerateGenericTableWithoutSplayingAvl.
#[unsafe(no_mangle)]
pub extern "system" fn RtlLookupFirstMatchingElementGenericTableAvl(
    table: *mut AvlTable,
    buffer: *const c_void,
    restart_key: *mut *mut c_void,
) -> *mut c_void {
    match find(table, buffer) {
        Ok(n) => {
            if !restart_key.is_null() {
                // SAFETY: the caller's key.
                unsafe { *restart_key = n.cast() };
            }
            element(n)
        }
        Err(_) => null_mut(),
    }
}
