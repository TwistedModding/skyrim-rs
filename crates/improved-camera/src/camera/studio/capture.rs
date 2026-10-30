//! `camera/studio/StudioCapture`: screenshots of the current framing, or of every camera bookmark
//! in turn, as PNGs in `PhotoMode\Shots`. The HUD is taken down a couple of frames ahead
//! (Scaleform acts on `menusVisible` at its next render), a bracket waits for the camera's move and
//! the world to settle, and the frame is read from the host before the overlay draws, so neither
//! the menu nor the HUD painters are in the shot.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::io::CameraBookmark;

/// Frames between hiding the HUD and the grab.
const HUD_SETTLE_FRAMES: u32 = 2;

/// The crop aspects, the composition guide's set (`Capture::Aspect`).
pub const ASPECT_NAMES: [&str; 5] = ["Native", "16:9", "21:9", "4:5", "1:1"];

/// `AspectRatio`: width over height, 0 for the frame's own.
#[must_use]
pub fn aspect_ratio(aspect: i32) -> f32 {
    match aspect {
        1 => 16.0 / 9.0,
        2 => 21.0 / 9.0,
        3 => 4.0 / 5.0,
        4 => 1.0,
        _ => 0.0,
    }
}

/// The centred `ratio` rectangle of a `width` x `height` frame: left, top, width, height.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
pub fn crop_rect(width: usize, height: usize, ratio: f32) -> (usize, usize, usize, usize) {
    if ratio <= 0.0 || height == 0 {
        return (0, 0, width, height);
    }
    if width as f32 / height as f32 > ratio {
        let w = ((height as f32 * ratio).round() as usize).min(width);
        ((width - w) / 2, 0, w, height)
    } else {
        let h = ((width as f32 / ratio).round() as usize).min(height);
        (0, (height - h) / 2, width, h)
    }
}

/// `TimeStamp`: `YYYYMMDD_HHMMSS` of `seconds` since 1970 (UTC; the mod has no time zone API).
#[must_use]
pub fn time_stamp(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let rest = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}_{:02}{:02}{:02}", rest / 3600, rest / 60 % 60, rest % 60)
}

/// `NextFileName`: the first `ICShot_<stamp>_<index>.png` from `index` on that does not exist.
#[must_use]
pub fn next_file(dir: &Path, stamp: &str, index: &mut u32) -> PathBuf {
    loop {
        *index += 1;
        let path = dir.join(format!("ICShot_{stamp}_{:02}.png", *index));
        if !path.exists() {
            return path;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Idle,
    /// HUD down, waiting on Scaleform and any queued camera move.
    Settle,
    /// This frame is the shot.
    Grab,
}

/// What the capture asks of the frame it runs in.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Take the HUD down (`ShowMenus(false)`) when it is up.
    HideHud,
    /// Fly the camera to this framing, then wait.
    Fly(CameraBookmark),
    /// Read the frame now and write it to this file.
    Grab(PathBuf),
    /// Give the HUD back if the capture took it.
    Finish,
}

#[derive(Debug, Default)]
pub struct Capture {
    stage: Stage,
    settle_frames: u32,
    settle_until: Option<Instant>,
    queue: Vec<CameraBookmark>,
    bracket: bool,
    shot_index: u32,
    stamp: String,
    pub last_shot: String,
    /// The capture took the HUD down.
    pub hid_hud: bool,
}

impl Capture {
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.stage != Stage::Idle
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.queue.len()
    }

    fn start(&mut self, queue: Vec<CameraBookmark>, bracket: bool) -> Vec<Step> {
        self.queue = queue;
        self.bracket = bracket;
        self.shot_index = 0;
        self.stamp = time_stamp(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()));
        self.begin_shot(Duration::ZERO)
    }

    /// `RequestShot`.
    pub fn request_shot(&mut self) -> Vec<Step> {
        if self.is_busy() { Vec::new() } else { self.start(Vec::new(), false) }
    }

    /// `RequestBracket`: one shot per bookmark.
    pub fn request_bracket(&mut self, shots: Vec<CameraBookmark>) -> Vec<Step> {
        if self.is_busy() || shots.is_empty() { Vec::new() } else { self.start(shots, true) }
    }

    /// `BeginShot`: the HUD down, the camera sent to the next bookmark of a bracket.
    fn begin_shot(&mut self, delay: Duration) -> Vec<Step> {
        let mut steps = vec![Step::HideHud];
        self.settle_frames = HUD_SETTLE_FRAMES;
        self.settle_until = None;
        if self.bracket
            && let Some(shot) = self.queue.first()
        {
            steps.push(Step::Fly(shot.clone()));
            self.settle_until = Some(Instant::now() + delay);
        }
        self.stage = Stage::Settle;
        steps
    }

    /// `Cancel` and `OnStudioExit`: a sequence in flight stops and gives the HUD back.
    pub fn cancel(&mut self) -> Vec<Step> {
        if !self.is_busy() {
            return Vec::new();
        }
        self.queue.clear();
        self.stage = Stage::Idle;
        self.bracket = false;
        vec![Step::Finish]
    }

    /// `OnFrame`, once a frame from a frame callback; `delay` is the bracket's settle time and
    /// `shots` the folder the files go to.
    pub fn on_frame(&mut self, delay: Duration, shots: &Path) -> Vec<Step> {
        match self.stage {
            Stage::Idle => Vec::new(),
            Stage::Settle => {
                self.settle_frames = self.settle_frames.saturating_sub(1);
                if self.settle_frames == 0 && self.settle_until.is_none_or(|t| Instant::now() >= t) {
                    self.stage = Stage::Grab;
                }
                Vec::new()
            }
            Stage::Grab => {
                let path = next_file(shots, &self.stamp, &mut self.shot_index);
                let mut steps = vec![Step::Grab(path)];
                if !self.queue.is_empty() {
                    self.queue.remove(0);
                }
                if self.bracket && !self.queue.is_empty() {
                    steps.extend(self.begin_shot(delay));
                } else {
                    self.stage = Stage::Idle;
                    self.bracket = false;
                    steps.push(Step::Finish);
                }
                steps
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Capture, CameraBookmark, Step, aspect_ratio, crop_rect, next_file, time_stamp};

    #[test]
    fn crops_centre_the_guide_aspect() {
        assert_eq!(crop_rect(1920, 1080, aspect_ratio(0)), (0, 0, 1920, 1080));
        assert_eq!(crop_rect(1920, 1080, aspect_ratio(4)), (420, 0, 1080, 1080));
        assert_eq!(crop_rect(1920, 1080, aspect_ratio(2)), (0, 128, 1920, 823));
        assert_eq!(crop_rect(1920, 1080, aspect_ratio(3)), (528, 0, 864, 1080));
    }

    #[test]
    fn stamps_are_utc_dates() {
        assert_eq!(time_stamp(0), "19700101_000000");
        assert_eq!(time_stamp(1_793_268_245), "20261029_100405");
        assert_eq!(time_stamp(951_782_400), "20000229_000000");
    }

    #[test]
    fn a_shot_hides_the_hud_waits_two_frames_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("ic-capture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = Capture::default();
        assert_eq!(c.request_shot(), [Step::HideHud]);
        assert!(c.request_shot().is_empty(), "busy");
        assert_eq!(c.on_frame(Duration::ZERO, &dir), []);
        assert_eq!(c.on_frame(Duration::ZERO, &dir), []);
        let steps = c.on_frame(Duration::ZERO, &dir);
        let Step::Grab(path) = &steps[0] else { panic!("{steps:?}") };
        assert!(path.file_name().unwrap().to_string_lossy().ends_with("_01.png"));
        assert_eq!(steps[1], Step::Finish);
        assert!(!c.is_busy());
        std::fs::write(dir.join(format!("ICShot_{}_02.png", c.stamp)), "").unwrap();
        let mut index = 1;
        assert!(next_file(&dir, &c.stamp, &mut index).to_string_lossy().ends_with("_03.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bracket_flies_to_each_bookmark_in_turn() {
        let dir = std::env::temp_dir();
        let shots = vec![CameraBookmark { name: "a".into(), ..CameraBookmark::default() }, CameraBookmark { name: "b".into(), ..CameraBookmark::default() }];
        let mut c = Capture::default();
        let steps = c.request_bracket(shots.clone());
        assert_eq!(steps, [Step::HideHud, Step::Fly(shots[0].clone())]);
        let mut grabs = 0;
        let mut flown = vec![];
        for _ in 0..10 {
            for step in c.on_frame(Duration::ZERO, &dir) {
                match step {
                    Step::Grab(_) => grabs += 1,
                    Step::Fly(b) => flown.push(b.name),
                    _ => {}
                }
            }
        }
        assert_eq!((grabs, flown), (2, vec!["b".to_owned()]));
        assert!(!c.is_busy());
        assert_eq!(c.cancel(), []);
    }
}
