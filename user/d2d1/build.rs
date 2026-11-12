//! d2d1.dll's functions programs import by ordinal: Office's EXCEL.EXE,
//! PPCORE.DLL and its other Direct2D users import #1 to #5 (the Office
//! survey, ROADMAP OF), in the numbering of Wine's d2d1.spec:
//! D2D1CreateFactory #1, D2D1MakeRotateMatrix #2, D2D1MakeSkewMatrix #3,
//! D2D1IsMatrixInvertible #4, D2D1InvertMatrix #5 (d2d1_test checks each
//! ordinal against its name, on Windows too).
fn main() {
    for (name, ordinal) in [
        ("D2D1CreateFactory", 1),
        ("D2D1MakeRotateMatrix", 2),
        ("D2D1MakeSkewMatrix", 3),
        ("D2D1IsMatrixInvertible", 4),
        ("D2D1InvertMatrix", 5),
        ("D2D1SinCos", 9),
        ("D2D1Tan", 10),
        ("D2D1Vec3Length", 11),
    ] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }
}
