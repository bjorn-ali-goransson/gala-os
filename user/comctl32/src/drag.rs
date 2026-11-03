//! Image-list drags (ImageList_BeginDrag, DragEnter, DragMove,
//! DragLeave, EndDrag, SetDragCursorImage), as documented: one drag at a
//! time per process, of an image of a list with a hot spot, entered into
//! a window, moved in its coordinates, left and ended. gala keeps the
//! drag's state (the calls succeed while a drag of an existing image is
//! going on) but does not draw the drag image over the window yet
//! (recorded once per drag): wxWidgets' drag-and-drop feedback works
//! without it.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};

static DRAGGING: AtomicBool = AtomicBool::new(false);
static LIST: AtomicUsize = AtomicUsize::new(0);
static IMAGE: AtomicI32 = AtomicI32::new(0);
static WINDOW: AtomicUsize = AtomicUsize::new(0);
static X: AtomicI32 = AtomicI32::new(0);
static Y: AtomicI32 = AtomicI32::new(0);

/// ImageList_BeginDrag: starts dragging image `i` of `list` with the hot
/// spot (`dx`, `dy`); FALSE if the image is not there.
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_BeginDrag(list: usize, i: i32, _dx: i32, _dy: i32) -> i32 {
    let count = crate::imagelist::ImageList_GetImageCount(list);
    if i < 0 || i >= count {
        return 0;
    }
    LIST.store(list, Ordering::Relaxed);
    IMAGE.store(i, Ordering::Relaxed);
    WINDOW.store(0, Ordering::Relaxed);
    DRAGGING.store(true, Ordering::Release);
    crate::unsupported(b"comctl32.dll!ImageList_BeginDrag(drag image not drawn)", 0);
    1
}

/// ImageList_DragEnter: the drag shown over `window` at (`x`, `y`).
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_DragEnter(window: usize, x: i32, y: i32) -> i32 {
    if !DRAGGING.load(Ordering::Acquire) {
        return 0;
    }
    WINDOW.store(window, Ordering::Relaxed);
    X.store(x, Ordering::Relaxed);
    Y.store(y, Ordering::Relaxed);
    1
}

/// ImageList_DragMove: the drag image moved to (`x`, `y`).
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_DragMove(x: i32, y: i32) -> i32 {
    if !DRAGGING.load(Ordering::Acquire) {
        return 0;
    }
    X.store(x, Ordering::Relaxed);
    Y.store(y, Ordering::Relaxed);
    1
}

/// ImageList_DragLeave: the drag image hidden from `window`.
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_DragLeave(_window: usize) -> i32 {
    WINDOW.store(0, Ordering::Relaxed);
    1
}

/// ImageList_EndDrag: the drag over.
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_EndDrag() {
    DRAGGING.store(false, Ordering::Release);
    WINDOW.store(0, Ordering::Relaxed);
    LIST.store(0, Ordering::Relaxed);
}

/// ImageList_SetDragCursorImage: combines image `i` of `list` (a cursor
/// image, hot spot (`dx`, `dy`)) with the drag image; FALSE if it is not
/// there or no drag is going on.
#[unsafe(no_mangle)]
pub extern "system" fn ImageList_SetDragCursorImage(
    list: usize,
    i: i32,
    _dx: i32,
    _dy: i32,
) -> i32 {
    if !DRAGGING.load(Ordering::Acquire) {
        return 0;
    }
    let count = crate::imagelist::ImageList_GetImageCount(list);
    i32::from(i >= 0 && i < count)
}
