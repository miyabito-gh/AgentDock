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

/// タイトルバーとみなす上端の高さ（物理ピクセル）。これが画面内に見えていなければ掴めない。
const TITLE_STRIP_PX: u32 = 32;

fn overlap_1d(a_start: i64, a_len: i64, b_start: i64, b_len: i64) -> i64 {
    ((a_start + a_len).min(b_start + b_len) - a_start.max(b_start)).max(0)
}

/// タイトルバー部分が、そのモニターの作業領域に十分見えているか（幅は `MIN_VISIBLE_PX` 以上、高さは半分以上）。
fn title_visible(b: &WindowBounds, m: &WorkArea) -> bool {
    let strip = TITLE_STRIP_PX.min(b.height) as i64;
    let w = overlap_1d(b.x as i64, b.width as i64, m.x as i64, m.width as i64);
    let h = overlap_1d(b.y as i64, strip, m.y as i64, m.height as i64);
    w >= MIN_VISIBLE_PX as i64 && h * 2 >= strip
}

/// 保存した位置がどのモニターにも十分に重ならなければ、主モニター（`monitors[0]`）の中央へ戻す。
/// サイズはモニターより大きければ縮める。`min` は窓の最小サイズ（モニターの作業領域のほうが小さければ作業領域に合わせる）。
/// モニターを取得できない（空）ときは位置を判断できないので、サイズの下限だけ守って返す。
pub fn fit_to_monitors(saved: WindowBounds, monitors: &[WorkArea], min: (u32, u32)) -> WindowBounds {
    let mut b = saved;
    b.width = b.width.max(min.0);
    b.height = b.height.max(min.1);
    let Some(primary) = monitors.first() else { return b };
    match monitors.iter().find(|m| title_visible(&b, m)) {
        Some(m) => {
            // 見えているモニターに収める（大きすぎれば縮め、はみ出していれば寄せる）。
            b.width = b.width.min(m.width);
            b.height = b.height.min(m.height);
            b.x = (b.x as i64).clamp(m.x as i64, m.x as i64 + (m.width - b.width) as i64) as i32;
            b.y = (b.y as i64).clamp(m.y as i64, m.y as i64 + (m.height - b.height) as i64) as i32;
            b
        }
        None => {
            b.width = b.width.min(primary.width);
            b.height = b.height.min(primary.height);
            b.x = primary.x + ((primary.width - b.width) / 2) as i32;
            b.y = primary.y + ((primary.height - b.height) / 2) as i32;
            b
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wa(x: i32, y: i32, w: u32, h: u32) -> WorkArea {
        WorkArea { x, y, width: w, height: h }
    }
    fn wb(x: i32, y: i32, w: u32, h: u32) -> WindowBounds {
        WindowBounds { x, y, width: w, height: h, maximized: false }
    }
    const MIN: (u32, u32) = (960, 600);

    #[test]
    fn visible_window_is_kept_as_is() {
        let m = [wa(0, 0, 1920, 1040)];
        assert_eq!(fit_to_monitors(wb(100, 80, 1280, 800), &m, MIN), wb(100, 80, 1280, 800));
    }

    #[test]
    fn offscreen_window_goes_to_the_center_of_the_primary_monitor() {
        let m = [wa(0, 0, 1920, 1040)];
        // 取り外した2台目にあった位置（右へ大きく外れている）。
        assert_eq!(fit_to_monitors(wb(2500, 100, 1280, 800), &m, MIN), wb(320, 120, 1280, 800));
        // 上へ外れてタイトルバーが見えない。
        assert_eq!(fit_to_monitors(wb(100, -700, 1280, 800), &m, MIN), wb(320, 120, 1280, 800));
    }

    #[test]
    fn window_on_a_second_monitor_stays_there() {
        let m = [wa(0, 0, 1920, 1040), wa(1920, 0, 1920, 1080)];
        assert_eq!(fit_to_monitors(wb(2100, 50, 1000, 700), &m, MIN), wb(2100, 50, 1000, 700));
        // 左側にある副モニター（負の座標）。
        let m = [wa(0, 0, 1920, 1040), wa(-1280, 0, 1280, 1000)];
        assert_eq!(fit_to_monitors(wb(-1200, 40, 1000, 700), &m, (800, 500)), wb(-1200, 40, 1000, 700));
    }

    #[test]
    fn barely_overlapping_window_is_relocated() {
        let m = [wa(0, 0, 1920, 1040)];
        // 右端に30pxだけ掛かっている（64px未満）。
        let got = fit_to_monitors(wb(1890, 100, 1000, 700), &m, MIN);
        assert_eq!((got.x, got.y), (460, 170));
        // 64pxちょうどは見えているとみなして残す（モニター内に寄せるだけ）。
        let got = fit_to_monitors(wb(1856, 100, 1000, 700), &m, MIN);
        assert_eq!(got.x, 920);
    }

    #[test]
    fn high_dpi_physical_pixels_and_oversize_are_handled() {
        // 4K・150%の物理ピクセル。保存サイズが作業領域より大きければ縮める。
        let m = [wa(0, 0, 3840, 2100)];
        assert_eq!(fit_to_monitors(wb(0, 0, 5000, 3000), &m, MIN), wb(0, 0, 3840, 2100));
        // 小さい画面では最小サイズより作業領域を優先する。
        let m = [wa(0, 0, 900, 560)];
        let got = fit_to_monitors(wb(10, 10, 1280, 800), &m, MIN);
        assert_eq!((got.width, got.height), (900, 560));
    }

    #[test]
    fn size_is_raised_to_the_minimum_and_maximized_flag_is_kept() {
        let m = [wa(0, 0, 1920, 1040)];
        let mut s = wb(100, 100, 300, 200);
        s.maximized = true;
        let got = fit_to_monitors(s, &m, MIN);
        assert_eq!((got.width, got.height, got.maximized), (960, 600, true));
        // モニター情報が空なら位置は触らない。
        assert_eq!(fit_to_monitors(wb(5, 6, 1000, 700), &[], MIN), wb(5, 6, 1000, 700));
    }
}
