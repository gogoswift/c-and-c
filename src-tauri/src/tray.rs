use image::{DynamicImage, GenericImageView, RgbaImage};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::image::Image as TauriImage;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

const FRAME_SIZE: u32 = 32;
const PIXEL_SCALE: u32 = 4;
const CANVAS_W: u32 = 17; // max(all anim widths)
const CANVAS_H: u32 = 11; // sit 底部裁掉 1px，sleep 底部对齐有少量空白
const ANIM_INTERVAL: u64 = 250; // 帧间隔 ms
const ACTIVITY_CHECK_INTERVAL: u64 = 5_000; // 活跃检测间隔 ms

/// 动画定义：精灵图中各动画的有效像素区域
struct AnimDef {
    #[allow(dead_code)]
    name: &'static str,
    row: u32,
    frames: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// cat2-sprite.png 各动画的有效像素区域（联合所有帧的边界）
/// 参数必须与 Electron 版 index.ts 中的 ANIMS 定义完全一致
const ANIMS: &[AnimDef] = &[
    AnimDef { name: "sit1",    row: 0, frames: 4, x: 9,  y: 20, w: 12, h: 12 },
    AnimDef { name: "sit2",    row: 1, frames: 4, x: 9,  y: 20, w: 12, h: 12 },
    AnimDef { name: "sit3",    row: 2, frames: 4, x: 9,  y: 20, w: 13, h: 12 },
    AnimDef { name: "sit4",    row: 3, frames: 4, x: 9,  y: 20, w: 13, h: 12 },
    AnimDef { name: "run1",    row: 4, frames: 8, x: 7,  y: 20, w: 17, h: 12 },
    AnimDef { name: "run2",    row: 5, frames: 8, x: 7,  y: 19, w: 17, h: 13 },
    AnimDef { name: "sleep",   row: 6, frames: 4, x: 8,  y: 24, w: 16, h: 8  }, // index 6
    AnimDef { name: "play",    row: 7, frames: 6, x: 10, y: 20, w: 15, h: 12 }, // index 7
    AnimDef { name: "pounce",  row: 8, frames: 7, x: 9,  y: 14, w: 15, h: 18 }, // index 8
    AnimDef { name: "stretch", row: 9, frames: 8, x: 7,  y: 21, w: 17, h: 11 }, // index 9
];

/// 活跃状态下的动画池（带权重和持续时间）
struct ActiveAnimEntry {
    index: usize,   // ANIMS 数组的索引
    weight: u32,     // 被选中的相对概率
    duration_ms: u64, // 播放持续时间 ms
}

/// 活跃状态下可随机切换的动画池（仅 sit 系列）
const ACTIVE_ANIMS: &[ActiveAnimEntry] = &[
    ActiveAnimEntry { index: 0, weight: 1, duration_ms: 15000 }, // sit1
    ActiveAnimEntry { index: 1, weight: 1, duration_ms: 15000 }, // sit2
    ActiveAnimEntry { index: 2, weight: 1, duration_ms: 15000 }, // sit3
    ActiveAnimEntry { index: 3, weight: 1, duration_ms: 15000 }, // sit4
];

/// 动画状态机三态
///
/// sleeping -> waking(stretch x2) -> active(随机切换)
/// active -> sleeping(当 cc/cx 都不活跃)
#[derive(Clone, Copy, PartialEq)]
enum TrayState {
    Sleeping, // cc/cx 都不活跃，播放 sleep 动画
    Waking,   // 刚检测到活跃，播放 stretch 动画 2 次
    Active,   // 活跃中，按权重随机切换动画
}

/// Tray 动画内部状态
struct TrayAnimState {
    state: TrayState,
    current_anim: usize,        // ANIMS 的索引
    frame_index: u32,           // 当前帧
    stretch_count: u32,         // stretch 已播放次数
    anim_start_ms: u64,         // 当前动画开始时间
    current_anim_duration: u64, // 当前动画应播放的时长
}

/// 预提取所有动画的所有帧 RGBA 数据（启动时执行一次）
fn precompute_frames(sprite: &DynamicImage) -> Vec<Vec<Vec<u8>>> {
    let mut all_frames = Vec::new();
    for anim in ANIMS {
        let mut frames = Vec::new();
        for f in 0..anim.frames {
            let frame = extract_frame(sprite, anim, f);
            frames.push(frame);
        }
        all_frames.push(frames);
    }
    all_frames
}

/// 从精灵图裁剪一帧，最近邻放大 2x，居中放入统一画布
///
/// 统一画布尺寸 = CANVAS_W x CANVAS_H（包含底部2px留白让猫往上移）
fn extract_frame(sprite: &DynamicImage, anim: &AnimDef, frame_idx: u32) -> Vec<u8> {
    let src_x = frame_idx * FRAME_SIZE + anim.x;
    let src_y = anim.row * FRAME_SIZE + anim.y;
    let sub = sprite.crop_imm(src_x, src_y, anim.w, anim.h);

    let scaled_w = anim.w * PIXEL_SCALE;
    let scaled_h = anim.h * PIXEL_SCALE;
    let canvas_w = CANVAS_W * PIXEL_SCALE;
    let canvas_h = CANVAS_H * PIXEL_SCALE;

    // 水平居中
    let offset_x = (canvas_w.saturating_sub(scaled_w)) / 2;
    // 垂直：底部对齐，超高动画裁剪顶部
    let offset_y = if anim.h >= CANVAS_H {
        0 // 动画比画布高 → 从顶部开始画（底部溢出自动裁剪）
    } else {
        (CANVAS_H - anim.h) * PIXEL_SCALE // 底部对齐
    };

    let mut canvas = RgbaImage::new(canvas_w, canvas_h);

    // 最近邻放大
    for dy in 0..scaled_h {
        for dx in 0..scaled_w {
            let sx = dx / PIXEL_SCALE;
            let sy = dy / PIXEL_SCALE;
            if sx < anim.w && sy < anim.h {
                let pixel = sub.get_pixel(sx, sy);
                let cx = offset_x + dx;
                let cy = offset_y + dy;
                if cx < canvas_w && cy < canvas_h {
                    canvas.put_pixel(cx, cy, pixel);
                }
            }
        }
    }

    canvas.into_raw()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 按权重随机选一个活跃动画（避免与当前动画重复），返回 (anim_index, duration)
fn weighted_random_anim(current: usize) -> (usize, u64) {
    // 排除当前动画
    let pool: Vec<&ActiveAnimEntry> = ACTIVE_ANIMS
        .iter()
        .filter(|a| a.index != current)
        .collect();

    let total_weight: u32 = pool.iter().map(|a| a.weight).sum();
    if total_weight == 0 {
        return (0, 15000);
    }

    // 用时间戳做简单随机（避免引入额外 crate）
    let mut r = (now_ms() % total_weight as u64) as u32;
    for aa in &pool {
        if r < aa.weight {
            return (aa.index, aa.duration_ms);
        }
        r -= aa.weight;
    }
    (pool[0].index, pool[0].duration_ms)
}

/// 检测 cc/cx 是否有活跃会话
fn check_has_active_session() -> bool {
    use crate::codex_discovery;
    use crate::session_discovery;
    let cc = session_discovery::discover_active_sessions();
    let cx = codex_discovery::discover_active_codex_sessions();
    !cc.is_empty() || !cx.is_empty()
}

/// 初始化 Tray 图标和动画状态机
///
/// 包含两个异步定时器：
/// 1. 帧动画定时器（250ms）- 播放当前动画帧
/// 2. 活跃状态检测定时器（5s）- 检测 cc/cx 会话活跃状态，驱动状态机转换
pub fn setup_tray(app: &AppHandle) {
    let resource_path = app
        .path()
        .resource_dir()
        .unwrap_or_default()
        .join("resources")
        .join("cat2-sprite.png");

    let sprite = match image::open(&resource_path) {
        Ok(s) => s,
        Err(e) => {
            log::error!("[tray] Failed to load cat2-sprite.png: {:?}", e);
            return;
        }
    };

    let all_frames = precompute_frames(&sprite);
    let canvas_w = CANVAS_W * PIXEL_SCALE;
    let canvas_h = CANVAS_H * PIXEL_SCALE;

    let anim_state = Arc::new(Mutex::new(TrayAnimState {
        state: TrayState::Sleeping,
        current_anim: 6, // sleep
        frame_index: 0,
        stretch_count: 0,
        anim_start_ms: now_ms(),
        current_anim_duration: 0,
    }));

    // 构建右键菜单
    let quit_item = MenuItemBuilder::with_id("quit", "退出").build(app).unwrap();
    let menu = MenuBuilder::new(app).item(&quit_item).build().unwrap();

    // 创建 Tray 图标，初始显示 sleep 动画第 0 帧
    let initial_frame = &all_frames[6][0];
    let tray = match TrayIconBuilder::new()
        .icon(TauriImage::new_owned(
            initial_frame.clone(),
            canvas_w,
            canvas_h,
        ))
        .tooltip("c&c")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "quit" {
                app.exit(0);
            }
        })
        .on_tray_icon_event(|tray_icon, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray_icon.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
        })
        .build(app)
    {
        Ok(t) => t,
        Err(e) => {
            log::error!("[tray] Failed to create tray icon: {:?}", e);
            return;
        }
    };

    let tray_id = tray.id().clone();

    // ── 帧动画定时器 (250ms) ──
    let app_handle = app.clone();
    let frames_clone = Arc::new(all_frames);
    let state_clone = anim_state.clone();
    let frames_for_anim = frames_clone.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(ANIM_INTERVAL));
            let frame_data = {
                let mut st = state_clone.lock().unwrap();
                let anim = &ANIMS[st.current_anim];
                st.frame_index = (st.frame_index + 1) % anim.frames;

                // waking 状态: 帧归零 = 一轮播完
                if st.state == TrayState::Waking && st.frame_index == 0 {
                    st.stretch_count += 1;
                    if st.stretch_count >= 2 {
                        // 伸懒腰播了 2 次，进入活跃状态
                        st.state = TrayState::Active;
                        let (anim_idx, dur) = weighted_random_anim(st.current_anim);
                        st.current_anim = anim_idx;
                        st.frame_index = 0;
                        st.anim_start_ms = now_ms();
                        st.current_anim_duration = dur;
                    }
                }

                frames_for_anim[st.current_anim][st.frame_index as usize].clone()
            };
            if let Some(tray) = app_handle.tray_by_id(&tray_id) {
                let _ = tray.set_icon(Some(TauriImage::new_owned(
                    frame_data, canvas_w, canvas_h,
                )));
            }
        }
    });

    // ── 活跃状态检测定时器 (5s) ──
    let state_clone2 = anim_state.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(ACTIVITY_CHECK_INTERVAL));
            let has_active = check_has_active_session();

            let mut st = state_clone2.lock().unwrap();
            let now = now_ms();

            match st.state {
                TrayState::Sleeping => {
                    if has_active {
                        // sleeping -> waking: 播放 stretch
                        st.state = TrayState::Waking;
                        st.current_anim = 9; // stretch
                        st.frame_index = 0;
                        st.stretch_count = 0;
                        st.anim_start_ms = now;
                    }
                }
                TrayState::Waking => {
                    if !has_active {
                        // 活跃消失 -> 回到 sleeping
                        st.state = TrayState::Sleeping;
                        st.current_anim = 6; // sleep
                        st.frame_index = 0;
                    }
                    // waking -> active 的转换由帧动画定时器处理
                }
                TrayState::Active => {
                    if !has_active {
                        // active -> sleeping
                        st.state = TrayState::Sleeping;
                        st.current_anim = 6; // sleep
                        st.frame_index = 0;
                    } else if now - st.anim_start_ms >= st.current_anim_duration {
                        // 活跃中，按当前动画的持续时间切换到下一个随机动画
                        let (anim_idx, dur) = weighted_random_anim(st.current_anim);
                        st.current_anim = anim_idx;
                        st.frame_index = 0;
                        st.anim_start_ms = now;
                        st.current_anim_duration = dur;
                    }
                }
            }
        }
    });
}
