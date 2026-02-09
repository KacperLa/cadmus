use crate::device::CURRENT_DEVICE;
use crate::geom::{elbow, nearest_segment_point, Axis, DiagDir, Dir, LinearDir, Point, Vec2};
use crate::input::{ButtonCode, ButtonStatus, DeviceEvent, FingerStatus};
use crate::unit::mm_to_px;
use crate::view::{Event, KeyboardEvent, TextKind};
use fxhash::FxHashMap;
use std::f64;
use std::fmt;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const TAP_JITTER_MM: f32 = 6.0;
pub const HOLD_JITTER_MM: f32 = 1.5;
pub const HOLD_DELAY_SHORT: Duration = Duration::from_millis(666);
pub const HOLD_DELAY_LONG: Duration = Duration::from_millis(1333);

#[derive(Debug, Copy, Clone)]
pub enum GestureEvent {
    Tap(Point),
    MultiTap([Point; 2]),
    Swipe {
        dir: Dir,
        start: Point,
        end: Point,
    },
    SlantedSwipe {
        dir: DiagDir,
        start: Point,
        end: Point,
    },
    MultiSwipe {
        dir: Dir,
        starts: [Point; 2],
        ends: [Point; 2],
    },
    Arrow {
        dir: Dir,
        start: Point,
        end: Point,
    },
    MultiArrow {
        dir: Dir,
        starts: [Point; 2],
        ends: [Point; 2],
    },
    Corner {
        dir: DiagDir,
        start: Point,
        end: Point,
    },
    MultiCorner {
        dir: DiagDir,
        starts: [Point; 2],
        ends: [Point; 2],
    },
    Pinch {
        axis: Axis,
        center: Point,
        factor: f32,
    },
    Spread {
        axis: Axis,
        center: Point,
        factor: f32,
    },
    Rotate {
        center: Point,
        quarter_turns: i8,
        angle: f32,
    },
    Cross(Point),
    Diamond(Point),
    HoldFingerShort(Point, i32),
    HoldFingerLong(Point, i32),
    HoldButtonShort(ButtonCode),
    HoldButtonLong(ButtonCode),
}

impl fmt::Display for GestureEvent {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            GestureEvent::Tap(pt) => write!(f, "Tap {}", pt),
            GestureEvent::MultiTap(pts) => write!(f, "Multitap {} {}", pts[0], pts[1]),
            GestureEvent::Swipe { dir, .. } => write!(f, "Swipe {}", dir),
            GestureEvent::SlantedSwipe { dir, .. } => write!(f, "SlantedSwipe {}", dir),
            GestureEvent::MultiSwipe { dir, .. } => write!(f, "Multiswipe {}", dir),
            GestureEvent::Arrow { dir, .. } => write!(f, "Arrow {}", dir),
            GestureEvent::MultiArrow { dir, .. } => write!(f, "Multiarrow {}", dir),
            GestureEvent::Corner { dir, .. } => write!(f, "Corner {}", dir),
            GestureEvent::MultiCorner { dir, .. } => write!(f, "Multicorner {}", dir),
            GestureEvent::Pinch {
                axis,
                center,
                factor,
                ..
            } => write!(f, "Pinch {} {} {:.2}", axis, center, factor),
            GestureEvent::Spread {
                axis,
                center,
                factor,
                ..
            } => write!(f, "Spread {} {} {:.2}", axis, center, factor),
            GestureEvent::Rotate {
                center,
                quarter_turns,
                ..
            } => write!(f, "Rotate {} {}", center, *quarter_turns as i32 * 90),
            GestureEvent::Cross(pt) => write!(f, "Cross {}", pt),
            GestureEvent::Diamond(pt) => write!(f, "Diamond {}", pt),
            GestureEvent::HoldFingerShort(pt, id) => write!(f, "Short-held finger {} {}", id, pt),
            GestureEvent::HoldFingerLong(pt, id) => write!(f, "Long-held finger {} {}", id, pt),
            GestureEvent::HoldButtonShort(code) => write!(f, "Short-held button {:?}", code),
            GestureEvent::HoldButtonLong(code) => write!(f, "Long-held button {:?}", code),
        }
    }
}

#[derive(Debug)]
pub struct TouchState {
    time: f64,
    held: bool,
    positions: Vec<Point>,
}

fn keycode_to_keyboard_event(code: u16, ctrl: bool, shift: bool) -> Option<KeyboardEvent> {
    match code {
        // Letters (a-z)
        30 => Some(KeyboardEvent::Append(if shift { 'A' } else { 'a' })),
        48 => Some(KeyboardEvent::Append(if shift { 'B' } else { 'b' })),
        46 => Some(KeyboardEvent::Append(if shift { 'C' } else { 'c' })),
        32 => Some(KeyboardEvent::Append(if shift { 'D' } else { 'd' })),
        18 => Some(KeyboardEvent::Append(if shift { 'E' } else { 'e' })),
        33 => Some(KeyboardEvent::Append(if shift { 'F' } else { 'f' })),
        34 => Some(KeyboardEvent::Append(if shift { 'G' } else { 'g' })),
        35 => Some(KeyboardEvent::Append(if shift { 'H' } else { 'h' })),
        23 => Some(KeyboardEvent::Append(if shift { 'I' } else { 'i' })),
        36 => Some(KeyboardEvent::Append(if shift { 'J' } else { 'j' })),
        37 => Some(KeyboardEvent::Append(if shift { 'K' } else { 'k' })),
        38 => Some(KeyboardEvent::Append(if shift { 'L' } else { 'l' })),
        50 => Some(KeyboardEvent::Append(if shift { 'M' } else { 'm' })),
        49 => Some(KeyboardEvent::Append(if shift { 'N' } else { 'n' })),
        24 => Some(KeyboardEvent::Append(if shift { 'O' } else { 'o' })),
        25 => Some(KeyboardEvent::Append(if shift { 'P' } else { 'p' })),
        16 => Some(KeyboardEvent::Append(if shift { 'Q' } else { 'q' })),
        19 => Some(KeyboardEvent::Append(if shift { 'R' } else { 'r' })),
        31 => Some(KeyboardEvent::Append(if shift { 'S' } else { 's' })),
        20 => Some(KeyboardEvent::Append(if shift { 'T' } else { 't' })),
        22 => Some(KeyboardEvent::Append(if shift { 'U' } else { 'u' })),
        47 => Some(KeyboardEvent::Append(if shift { 'V' } else { 'v' })),
        17 => Some(KeyboardEvent::Append(if shift { 'W' } else { 'w' })),
        45 => Some(KeyboardEvent::Append(if shift { 'X' } else { 'x' })),
        21 => Some(KeyboardEvent::Append(if shift { 'Y' } else { 'y' })),
        44 => Some(KeyboardEvent::Append(if shift { 'Z' } else { 'z' })),
        
        // Numbers
        2  => Some(KeyboardEvent::Append(if shift { '!' } else { '1' })),
        3  => Some(KeyboardEvent::Append(if shift { '@' } else { '2' })),
        4  => Some(KeyboardEvent::Append(if shift { '#' } else { '3' })),
        5  => Some(KeyboardEvent::Append(if shift { '$' } else { '4' })),
        6  => Some(KeyboardEvent::Append(if shift { '%' } else { '5' })),
        7  => Some(KeyboardEvent::Append(if shift { '^' } else { '6' })),
        8  => Some(KeyboardEvent::Append(if shift { '&' } else { '7' })),
        9  => Some(KeyboardEvent::Append(if shift { '*' } else { '8' })),
        10 => Some(KeyboardEvent::Append(if shift { '(' } else { '9' })),
        11 => Some(KeyboardEvent::Append(if shift { ')' } else { '0' })),
        
        57 => Some(KeyboardEvent::Append(' ')),
        28 => Some(KeyboardEvent::Submit),
        15 => Some(KeyboardEvent::Raw(b"\t")),
        1  => Some(KeyboardEvent::Raw(b"\x1b")),
        
        14 => Some(KeyboardEvent::Delete { target: TextKind::Char, dir: LinearDir::Backward }),
        
        103 => Some(KeyboardEvent::Raw(b"\x1b[A")),
        108 => Some(KeyboardEvent::Raw(b"\x1b[B")),
        106 => Some(KeyboardEvent::Raw(b"\x1b[C")),
        105 => Some(KeyboardEvent::Raw(b"\x1b[D")),
        
        12 => Some(KeyboardEvent::Append(if shift { '_' } else { '-' })),
        13 => Some(KeyboardEvent::Append(if shift { '+' } else { '=' })),
        26 => Some(KeyboardEvent::Append(if shift { '{' } else { '[' })),
        27 => Some(KeyboardEvent::Append(if shift { '}' } else { ']' })),
        43 => Some(KeyboardEvent::Append(if shift { '|' } else { '\\' })),
        39 => Some(KeyboardEvent::Append(if shift { ':' } else { ';' })),
        40 => Some(KeyboardEvent::Append(if shift { '"' } else { '\'' })),
        41 => Some(KeyboardEvent::Append(if shift { '~' } else { '`' })),
        51 => Some(KeyboardEvent::Append(if shift { '<' } else { ',' })),
        52 => Some(KeyboardEvent::Append(if shift { '>' } else { '.' })),
        53 => Some(KeyboardEvent::Append(if shift { '?' } else { '/' })),
        
        _ => None,
    }.map(|ke| {
        if ctrl {
            match ke {
                KeyboardEvent::Append(ch) if ch.is_ascii_alphabetic() => {
                    KeyboardEvent::Control(ch)
                }
                _ => ke,
            }
        } else {
            ke
        }
    })
}

pub fn gesture_events(rx: Receiver<DeviceEvent>) -> Receiver<Event> {
    let (ty, ry) = mpsc::channel();
    thread::spawn(move || parse_gesture_events(&rx, &ty));
    ry
}

pub fn parse_gesture_events(rx: &Receiver<DeviceEvent>, ty: &Sender<Event>) {
    let contacts: Arc<Mutex<FxHashMap<i32, TouchState>>> =
        Arc::new(Mutex::new(FxHashMap::default()));
    let buttons: Arc<Mutex<FxHashMap<ButtonCode, f64>>> =
        Arc::new(Mutex::new(FxHashMap::default()));
    let segments: Arc<Mutex<Vec<Vec<Point>>>> = Arc::new(Mutex::new(Vec::new()));
    let tap_jitter = mm_to_px(TAP_JITTER_MM, CURRENT_DEVICE.dpi);
    let hold_jitter = mm_to_px(HOLD_JITTER_MM, CURRENT_DEVICE.dpi);

    while let Ok(evt) = rx.recv() {
        ty.send(Event::Device(evt)).ok();
        match evt {
            DeviceEvent::Finger {
                status: FingerStatus::Down,
                position,
                id,
                time,
            } => {
                let mut ct = contacts.lock().unwrap();
                ct.insert(
                    id,
                    TouchState {
                        time,
                        held: false,
                        positions: vec![position],
                    },
                );
                let ty = ty.clone();
                let contacts = contacts.clone();
                let segments = segments.clone();
                thread::spawn(move || {
                    let mut held = false;
                    thread::sleep(HOLD_DELAY_SHORT);
                    {
                        let mut ct = contacts.lock().unwrap();
                        let sg = segments.lock().unwrap();
                        if ct.len() > 1 || !sg.is_empty() {
                            return;
                        }
                        if let Some(ts) = ct.get(&id) {
                            let tp = &ts.positions;
                            if (ts.time - time).abs() < f64::EPSILON
                                && (tp[tp.len() - 1] - position).length() < hold_jitter
                                && (tp[tp.len() / 2] - position).length() < hold_jitter
                            {
                                held = true;
                                ty.send(Event::Gesture(GestureEvent::HoldFingerShort(
                                    position, id,
                                )))
                                .ok();
                            }
                        }
                        if held {
                            if let Some(ts) = ct.get_mut(&id) {
                                ts.held = true;
                            }
                        } else {
                            return;
                        }
                    }
                    thread::sleep(HOLD_DELAY_LONG - HOLD_DELAY_SHORT);
                    {
                        let mut ct = contacts.lock().unwrap();
                        let sg = segments.lock().unwrap();
                        if ct.len() > 1 || !sg.is_empty() {
                            return;
                        }
                        if let Some(ts) = ct.get_mut(&id) {
                            let tp = &ts.positions;
                            if (ts.time - time).abs() < f64::EPSILON
                                && (tp[tp.len() - 1] - position).length() < hold_jitter
                                && (tp[tp.len() / 2] - position).length() < hold_jitter
                            {
                                ty.send(Event::Gesture(GestureEvent::HoldFingerLong(position, id)))
                                    .ok();
                            }
                        }
                    }
                });
            }
            DeviceEvent::Finger {
                status: FingerStatus::Motion,
                position,
                id,
                ..
            } => {
                let mut ct = contacts.lock().unwrap();
                if let Some(ref mut ts) = ct.get_mut(&id) {
                    ts.positions.push(position);
                }
            }
            DeviceEvent::Finger {
                status: FingerStatus::Up,
                position,
                id,
                ..
            } => {
                let mut ct = contacts.lock().unwrap();
                let mut sg = segments.lock().unwrap();
                if let Some(mut ts) = ct.remove(&id) {
                    if !ts.held {
                        ts.positions.push(position);
                        sg.push(ts.positions);
                    }
                }
                if ct.is_empty() && !sg.is_empty() {
                    let len = sg.len();
                    if len == 1 {
                        ty.send(Event::Gesture(interpret_segment(
                            &sg.pop().unwrap(),
                            tap_jitter,
                        )))
                        .ok();
                    } else if len == 2 {
                        let ge1 = interpret_segment(&sg.pop().unwrap(), tap_jitter);
                        let ge2 = interpret_segment(&sg.pop().unwrap(), tap_jitter);
                        match (ge1, ge2) {
                            (GestureEvent::Tap(c1), GestureEvent::Tap(c2)) => {
                                ty.send(Event::Gesture(GestureEvent::MultiTap([c1, c2])))
                                    .ok();
                            }
                            (
                                GestureEvent::Swipe {
                                    dir: d1,
                                    start: s1,
                                    end: e1,
                                    ..
                                },
                                GestureEvent::Swipe {
                                    dir: d2,
                                    start: s2,
                                    end: e2,
                                    ..
                                },
                            ) if d1 == d2 => {
                                ty.send(Event::Gesture(GestureEvent::MultiSwipe {
                                    dir: d1,
                                    starts: [s1, s2],
                                    ends: [e1, e2],
                                }))
                                .ok();
                            }
                            (
                                GestureEvent::Swipe {
                                    dir: d1,
                                    start: s1,
                                    end: e1,
                                    ..
                                },
                                GestureEvent::Swipe {
                                    dir: d2,
                                    start: s2,
                                    end: e2,
                                    ..
                                },
                            ) if d1 == d2.opposite() => {
                                let center = (s1 + s2) / 2;
                                let ds = (s2 - s1).length();
                                let de = (e2 - e1).length();
                                let factor = de / ds;
                                if factor < 1.0 {
                                    ty.send(Event::Gesture(GestureEvent::Pinch {
                                        axis: d1.axis(),
                                        center,
                                        factor,
                                    }))
                                    .ok();
                                } else {
                                    ty.send(Event::Gesture(GestureEvent::Spread {
                                        axis: d1.axis(),
                                        center,
                                        factor,
                                    }))
                                    .ok();
                                }
                            }
                            (
                                GestureEvent::SlantedSwipe {
                                    dir: d1,
                                    start: s1,
                                    end: e1,
                                    ..
                                },
                                GestureEvent::SlantedSwipe {
                                    dir: d2,
                                    start: s2,
                                    end: e2,
                                    ..
                                },
                            ) if d1 == d2.opposite() => {
                                let center = (s1 + s2) / 2;
                                let ds = (s2 - s1).length();
                                let de = (e2 - e1).length();
                                let factor = de / ds;
                                if factor < 1.0 {
                                    ty.send(Event::Gesture(GestureEvent::Pinch {
                                        axis: Axis::Diagonal,
                                        center,
                                        factor,
                                    }))
                                    .ok();
                                } else {
                                    ty.send(Event::Gesture(GestureEvent::Spread {
                                        axis: Axis::Diagonal,
                                        center,
                                        factor,
                                    }))
                                    .ok();
                                }
                            }
                            (
                                GestureEvent::Arrow {
                                    dir: Dir::East,
                                    start: s1,
                                    end: e1,
                                },
                                GestureEvent::Arrow {
                                    dir: Dir::West,
                                    start: s2,
                                    end: e2,
                                },
                            )
                            | (
                                GestureEvent::Arrow {
                                    dir: Dir::West,
                                    start: s2,
                                    end: e2,
                                },
                                GestureEvent::Arrow {
                                    dir: Dir::East,
                                    start: s1,
                                    end: e1,
                                },
                            ) if s1.x < s2.x => {
                                ty.send(Event::Gesture(GestureEvent::Cross(
                                    (s1 + e1 + s2 + e2) / 4,
                                )))
                                .ok();
                            }
                            (
                                GestureEvent::Arrow {
                                    dir: Dir::West,
                                    start: s1,
                                    end: e1,
                                },
                                GestureEvent::Arrow {
                                    dir: Dir::East,
                                    start: s2,
                                    end: e2,
                                },
                            )
                            | (
                                GestureEvent::Arrow {
                                    dir: Dir::East,
                                    start: s2,
                                    end: e2,
                                },
                                GestureEvent::Arrow {
                                    dir: Dir::West,
                                    start: s1,
                                    end: e1,
                                },
                            ) if s1.x < s2.x => {
                                ty.send(Event::Gesture(GestureEvent::Diamond(
                                    (s1 + e1 + s2 + e2) / 4,
                                )))
                                .ok();
                            }
                            (
                                GestureEvent::Arrow {
                                    dir: d1,
                                    start: s1,
                                    end: e1,
                                },
                                GestureEvent::Arrow {
                                    dir: d2,
                                    start: s2,
                                    end: e2,
                                },
                            ) if d1 == d2 => {
                                ty.send(Event::Gesture(GestureEvent::MultiArrow {
                                    dir: d1,
                                    starts: [s1, s2],
                                    ends: [e1, e2],
                                }))
                                .ok();
                            }
                            (
                                GestureEvent::Corner {
                                    dir: d1,
                                    start: s1,
                                    end: e1,
                                },
                                GestureEvent::Corner {
                                    dir: d2,
                                    start: s2,
                                    end: e2,
                                },
                            ) if d1 == d2 => {
                                ty.send(Event::Gesture(GestureEvent::MultiCorner {
                                    dir: d1,
                                    starts: [s1, s2],
                                    ends: [e1, e2],
                                }))
                                .ok();
                            }
                            (
                                GestureEvent::Tap(c),
                                GestureEvent::Swipe {
                                    start: s, end: e, ..
                                },
                            )
                            | (
                                GestureEvent::Swipe {
                                    start: s, end: e, ..
                                },
                                GestureEvent::Tap(c),
                            )
                            | (
                                GestureEvent::Tap(c),
                                GestureEvent::Arrow {
                                    start: s, end: e, ..
                                },
                            )
                            | (
                                GestureEvent::Arrow {
                                    start: s, end: e, ..
                                },
                                GestureEvent::Tap(c),
                            )
                            | (
                                GestureEvent::Tap(c),
                                GestureEvent::Corner {
                                    start: s, end: e, ..
                                },
                            )
                            | (
                                GestureEvent::Corner {
                                    start: s, end: e, ..
                                },
                                GestureEvent::Tap(c),
                            ) => {
                                // Angle are positive in the counter clockwise direction.
                                let angle = ((e - c).angle() - (s - c).angle()).to_degrees();
                                let quarter_turns = (angle / 90.0).round() as i8;
                                ty.send(Event::Gesture(GestureEvent::Rotate {
                                    angle,
                                    quarter_turns,
                                    center: c,
                                }))
                                .ok();
                            }
                            _ => (),
                        }
                    } else {
                        sg.clear();
                    }
                }
            }
            DeviceEvent::Button {
                status: ButtonStatus::Pressed,
                code,
                time,
            } => {
                // Convert keyboard raw events to keyboard events
                if let ButtonCode::KeyboardRaw { code: keycode, ctrl, shift, .. } = code {
                    if let Some(ke) = keycode_to_keyboard_event(keycode, ctrl, shift) {
                        ty.send(Event::Keyboard(ke)).ok();
                    }
                } else {
                    let mut bt = buttons.lock().unwrap();
                    bt.insert(code, time);
                    let ty = ty.clone();
                    let buttons = buttons.clone();
                    thread::spawn(move || {
                        thread::sleep(HOLD_DELAY_SHORT);
                        {
                            let bt = buttons.lock().unwrap();
                            if let Some(&initial_time) = bt.get(&code) {
                                if (initial_time - time).abs() < f64::EPSILON {
                                    ty.send(Event::Gesture(GestureEvent::HoldButtonShort(code)))
                                        .ok();
                                }
                            }
                        }
                        thread::sleep(HOLD_DELAY_LONG - HOLD_DELAY_SHORT);
                        {
                            let bt = buttons.lock().unwrap();
                            if let Some(&initial_time) = bt.get(&code) {
                                if (initial_time - time).abs() < f64::EPSILON {
                                    ty.send(Event::Gesture(GestureEvent::HoldButtonLong(code)))
                                        .ok();
                                }
                            }
                        }
                    });
                }
            }
            DeviceEvent::Button {
                status: ButtonStatus::Released,
                code,
                ..
            } => {
                let mut bt = buttons.lock().unwrap();
                bt.remove(&code);
            }
            _ => (),
        }
    }
}

fn interpret_segment(sp: &[Point], tap_jitter: f32) -> GestureEvent {
    let a = sp[0];
    let b = sp[sp.len() - 1];
    let ab = b - a;
    let d = ab.length();
    if d < tap_jitter {
        GestureEvent::Tap(a)
    } else {
        let p = sp[elbow(sp)];
        let (n, p) = {
            let p: Vec2 = p.into();
            let (n, _) = nearest_segment_point(p, a.into(), b.into());
            (n, p)
        };
        let np = p - n;
        let ds = np.length();
        if ds > d / 5.0 {
            let g = (np.x as f32 / np.y as f32).abs();
            if g < 0.5 || g > 2.0 {
                GestureEvent::Arrow {
                    dir: np.dir(),
                    start: a,
                    end: b,
                }
            } else {
                GestureEvent::Corner {
                    dir: np.diag_dir(),
                    start: a,
                    end: b,
                }
            }
        } else {
            let g = (ab.x as f32 / ab.y as f32).abs();
            if g < 0.5 || g > 2.0 {
                GestureEvent::Swipe {
                    start: a,
                    end: b,
                    dir: ab.dir(),
                }
            } else {
                GestureEvent::SlantedSwipe {
                    start: a,
                    end: b,
                    dir: ab.diag_dir(),
                }
            }
        }
    }
}
