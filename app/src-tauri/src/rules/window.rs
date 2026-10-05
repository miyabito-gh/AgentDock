//! 窓の位置・サイズの復元（P6、M37）。純粋ロジック。

use crate::backend::local::WindowBounds;

/// モニターの作業領域（タスクバーを除く、物理ピクセル）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 窓のタイトルバーが見えているとみなす最小の重なり（物理ピクセル）。
pub const MIN_VISIBLE_PX: u32 = 64;

/// 保存した位置がどのモニターにも十分に重ならなければ、主モニター（`monitors[0]`）の中央へ戻す。
/// サイズはモニターより大きければ縮める。`min` は窓の最小サイズ。
pub fn fit_to_monitors(saved: WindowBounds, monitors: &[WorkArea], min: (u32, u32)) -> WindowBounds {
    let _ = (saved, monitors, min);
    todo!("P6")
}
