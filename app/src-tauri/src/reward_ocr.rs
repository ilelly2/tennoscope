//! Read the four relic reward cards off the screen.
//!
//! This is the only source for the four cards. Memory cannot attribute them: the rewards are
//! resident, but nothing observed links them to a player or a screen slot, and that held whether
//! the local player was host or client. There is no per-player response record for anyone but the
//! local player, pointers to the four reward strings never cluster, and the display names sit far
//! apart with no ordered buffer.
//!
//! Reading the screen sidesteps attribution entirely, because the cards are already in screen
//! order. It is not general OCR either: EE.log names the squad's relics before the screen renders,
//! so each card only has to be matched to the nearest of roughly two dozen known rewards. A
//! garbled read still lands on the right item, which is what makes this trustworthy enough to be
//! published rather than guessed at. Only a near-exact read may name a reward outside that pool,
//! which is how a card from a relic with no known rewards still gets named.

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

use image::{DynamicImage, GenericImageView, GrayImage};

use warframe_acquisition::RewardCatalogEntry;

use crate::{
    overlay_window::WindowRect,
    reward_source::{CardCandidates, VisualRewardSource},
};

static LATEST_MATCHED_RECT: std::sync::Mutex<Option<WindowRect>> = std::sync::Mutex::new(None);

pub(crate) fn latest_matched_rect() -> Option<WindowRect> {
    *LATEST_MATCHED_RECT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn set_matched_rect(snapshot: &std::sync::Mutex<Option<WindowRect>>, rect: WindowRect) {
    *snapshot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(rect);
}

pub(crate) fn publish_latest_matched_rect(rect: WindowRect) {
    set_matched_rect(&LATEST_MATCHED_RECT, rect);
}

fn clear_matched_rect(snapshot: &std::sync::Mutex<Option<WindowRect>>) {
    *snapshot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

pub(crate) fn clear_latest_matched_rect() {
    clear_matched_rect(&LATEST_MATCHED_RECT);
}

/// Card geometry, calibrated from a labelled 1920x1080 reward screen: four cards on a 242px pitch
/// from x=478, i.e. a block centred on x=960.
///
/// These are fractions of window *height*, offset from the horizontal centre, not fractions of
/// width. Warframe scales its HUD with height and centres it horizontally, so a card's distance
/// from the centre is a fixed multiple of the window height at every aspect ratio. Fractions of
/// width only look right because they agree with these at 16:9, and disagree everywhere else.
const CARD_PITCH: f32 = 242.0 / 1080.0;
const CARD_WIDTH: f32 = 240.0 / 1080.0;
/// Centre of the card block, as a signed fraction of height from the window's horizontal centre.
/// The 1920x1080 block spans x=478 to x=1444, whose centre is x=961, one pixel right of the
/// screen centre, which is measurement noise, not an offset. Keeping the measured value rather
/// than rounding it to zero is what makes the 1920x1080 calibration reproduce exactly.
const BLOCK_CENTRE: f32 = (478.0 + (242.0 * 3.0 + 240.0) / 2.0 - 960.0) / 1080.0;

/// The four-card block, for anything that needs to sit against the cards rather than read them.
///
/// The overlay used to invent its own rectangle (75% of the screen wide, 56% of the way down)
/// while this module had the cards measured to the pixel. Two independent guesses at one rectangle
/// is why the overlay was half again as wide as the cards and about 75px below them. There is one
/// definition now, and it is this one, because this is the one that is calibrated.
///
/// `BOTTOM` is the underside of the player-name row, measured at y=525 on the 2026-07-27 host
/// screen, with a few pixels of clearance. Vertical fractions were always fractions of height, so
/// this one needed no correction.
pub const CARD_BLOCK_BOTTOM: f32 = 530.0 / 1080.0;

/// A full squad, and the layout the fractions above are calibrated against.
pub const MAX_CARDS: usize = 4;

/// Left edge of the card block for a squad of `cards`, in pixels from the window's left edge.
///
/// Warframe centres the block on however many cards it has, so dropping a card pulls both edges in
/// by half a pitch. That is not a detail: on a three-card screen every card sits 121px right of
/// where a four-card reader looks, which is enough for slot 0's crop to straddle the gutter and cut
/// the first title in half. Centring the block is the same statement, and it is the one that keeps
/// holding when the aspect ratio changes.
pub fn card_block_left(cards: usize, width: u32, height: u32) -> f32 {
    width as f32 / 2.0 + BLOCK_CENTRE * height as f32 - card_block_width(cards, height) / 2.0
}

/// Width of the card block for a squad of `cards`, in pixels.
pub fn card_block_width(cards: usize, height: u32) -> f32 {
    (CARD_PITCH * cards.saturating_sub(1) as f32 + CARD_WIDTH) * height as f32
}

/// The title band, measured against three captured reward screens on 2026-07-27.
///
/// This box was y=418 high 76, which was wrong at both edges. The top clipped the ascenders off
/// the first line of a two-line title, and clipped glyphs do not read as noise: they read as
/// confident wrong letters, so `Caliban Prime Chassis` came back as `Caliban Flime Gnassis`
/// (`C`->`G`, `h`->`n`) and the closed-set match had to absorb damage that was never in the pixels
/// on screen. The bottom reached past the title into the divider ornament below each card, which
/// tesseract read as a trailing `4` or `ty` on every single card, costing every read an edit.
///
/// y=408 clears the tallest ascender and height=58 stops above the divider. Thresholding cannot
/// substitute for this: clipped pixels are not on the screen to be recovered.
const TITLE_TOP: f32 = 408.0 / 1080.0;
const TITLE_HEIGHT: f32 = 58.0 / 1080.0;

/// Below this a read is still published, but its crop is kept for diagnosis. Every labelled card
/// reads exactly once the title is separated from the card art, so anything under 0.85 is an
/// anomaly worth having the pixels for.
///
/// This is debug-only crop retention, not an admission band. It must never be merged with the
/// trusted band: the trusted band decides whether a name may be published at all, while this one
/// only decides whether the pixels are worth keeping.
#[cfg(debug_assertions)]
const CROP_KEEP_BELOW: f32 = 0.85;

/// Distinguishes the crop files of concurrent readers.
///
/// Two readers are live at once whenever the log-triggered retry overlaps the poller, which is
/// exactly during the reward screen. Sharing one crop path means each deletes the other's file
/// mid-read, so the reads fail precisely when they are needed. Only the crop still touches the
/// disk, since it is what tesseract is handed, because the capture itself now stays in memory.
static SCRATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn scratch_file(kind: &str, extension: &str) -> PathBuf {
    let ticket = SCRATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "tennoscope-{kind}-{}-{ticket}.{extension}",
        std::process::id()
    ))
}

/// The screen reader holds one capture object across its reads. The object itself is cheap; keeping
/// it here preserves per-source geometry-change suppression while each read still locates the
/// Warframe window and its current monitor again.
pub struct ScreenRewardSource {
    capture: crate::reward_capture::GameCapture,
}

impl Default for ScreenRewardSource {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenRewardSource {
    pub fn new() -> Self {
        Self {
            capture: crate::reward_capture::GameCapture::new(),
        }
    }
}

impl VisualRewardSource for ScreenRewardSource {
    fn choices(
        &mut self,
        candidates: CardCandidates<'_>,
    ) -> Result<Vec<(String, f32)>, &'static str> {
        let frames = self.capture.capture_candidates()?;
        read_capture_candidates(&frames, &LATEST_MATCHED_RECT, |frame| {
            read_cards_in(frame, candidates)
        })
    }
}

fn read_capture_candidates<T>(
    candidates: &[crate::reward_capture::CapturedFrame],
    matched_rect: &std::sync::Mutex<Option<WindowRect>>,
    mut read: impl FnMut(&DynamicImage) -> Result<T, &'static str>,
) -> Result<T, &'static str> {
    let mut last_reason = "no Warframe window found";
    for candidate in candidates {
        match read(&candidate.image) {
            Ok(value) => {
                set_matched_rect(matched_rect, candidate.rect);
                return Ok(value);
            }
            Err(reason) => last_reason = reason,
        }
    }
    Err(last_reason)
}

/// Read the card titles out of a reward-screen image and match each with [`best_card_match`].
///
/// Returns each card with the score it matched at. The score is published as the card's
/// confidence, and it is what makes the crop geometry testable: a box that clips the title still
/// lands on the right reward through the closed-set match, so a name-only assertion passes
/// against a misaligned crop and proves nothing.
///
/// How many cards there are is not knowable ahead of time. It is the squad size, and EE.log only
/// says so after the screen has already come and gone, so the layouts are simply tried. Each
/// wrong one costs a single crop, because the read stops at the first card that will not match.
///
/// Widest first, and that ordering is load-bearing: a two-card block sits exactly over a four-card
/// block's middle two cards, so a four-card screen reads perfectly clean as "two cards" and would
/// quietly lose half the rewards if two were tried first. A solo run is not tried at all: one card
/// sits where a three-card screen's middle card sits, and a single reward is not a choice worth
/// advising on anyway.
///
/// Split out from the capture so it can be exercised against a real labelled screen instead of
/// only against a live game.
pub fn read_cards(
    image: &Path,
    candidates: CardCandidates<'_>,
) -> Result<Vec<(String, f32)>, &'static str> {
    let frame = image::open(image).map_err(|_| "capture could not be decoded")?;
    read_cards_in(&frame, candidates)
}

/// The same read against an already-decoded frame, which is what the live path has. Screen
/// capture stays in memory.
pub fn read_cards_in(
    image: &DynamicImage,
    candidates: CardCandidates<'_>,
) -> Result<Vec<(String, f32)>, &'static str> {
    if candidates.pool.is_empty() && candidates.catalog.is_empty() {
        return Err("no reward candidates");
    }
    let (width, height) = image.dimensions();
    // Up to three layouts per poll rather than one, so a poll off the reward screen costs
    // three crops instead of one, about 200ms every two seconds. Narrow it by asking the log for
    // the squad size if that ever shows up in a profile.
    let widest = read_cards_at(image, width, height, MAX_CARDS, candidates);
    if widest.is_ok() {
        return widest.map_err(|(_, reason)| reason);
    }
    // Ordering alone does not settle the four-against-two ambiguity, because it is the same pixels
    // either way: "two cards" and "four cards whose outer two would not match" are indistinguishable
    // at the two positions they share. The four-card slot 0 is the tiebreak. Blank means there is
    // genuinely nothing out there and the block really is narrower; any text at all means a wider
    // screen with a pool gap, and publishing its middle two as the whole screen would be a confident
    // half-answer. That case fails closed, exactly as it did before there was anything to guess.
    let outside_the_block_is_empty = matches!(&widest, Err((0, BLANK_CARD)));
    for cards in (2..MAX_CARDS).rev() {
        // Dropping one card shifts the block half a pitch, so a layout two cards narrower is shifted
        // a whole pitch and its slots land exactly on the widest layout's. Those are the ambiguous
        // ones, and only those need the tiebreak.
        let shares_slots_with_the_widest = (MAX_CARDS - cards) % 2 == 0;
        if shares_slots_with_the_widest && !outside_the_block_is_empty {
            break;
        }
        if let Ok(read) = read_cards_at(image, width, height, cards, candidates) {
            return Ok(read);
        }
    }
    widest.map_err(|(_, reason)| reason)
}

const BLANK_CARD: &str = "a reward card read as blank";

/// The card's text did not identify a reward.
///
/// This exact string is the contract with `reward_recognition`, which treats it and the blank as
/// the two routine misses and warns immediately on anything else. A card that named nothing
/// because the read was ambiguous is still a routine miss, so it carries this string and not a
/// new one.
const POOL_MISS: &str = "reward card text did not match the relic pool";

/// Reads the `cards` title slots of one layout, stopping at the first that will not match. The
/// failing slot comes back with the reason because `read_cards` needs to know whether the *first*
/// slot was the one that failed, which is what tells a misplaced block apart from a pool gap.
fn read_cards_at(
    image: &DynamicImage,
    width: u32,
    height: u32,
    cards: usize,
    candidates: CardCandidates<'_>,
) -> Result<Vec<(String, f32)>, (usize, &'static str)> {
    let left = card_block_left(cards, width, height);
    let mut read = Vec::with_capacity(cards);
    for slot in 0..cards {
        let (text, crop) = read_region(
            image,
            (left + CARD_PITCH * slot as f32 * height as f32) as u32,
            (TITLE_TOP * height as f32) as u32,
            (CARD_WIDTH * height as f32) as u32,
            (TITLE_HEIGHT * height as f32) as u32,
        )
        .map_err(|reason| (slot, reason))?;
        let reading = resolve_card(&text, candidates);
        // Without the raw text a failed read is unattributable: reading the wrong place, reading a
        // screen that is not the reward screen, and reading a card whose name is not in the pool
        // all surface as the same error. The text alone is not enough either: a misplaced crop
        // yields clean-looking wrong words rather than obvious garbage, which is how the title box
        // stayed 10px too low for five live runs. Keep the pixels behind a poor read.
        #[cfg(debug_assertions)]
        let keep_crop = match &reading {
            CardReading::Named { score, .. } => *score < CROP_KEEP_BELOW,
            _ => true,
        };
        #[cfg(not(debug_assertions))]
        let keep_crop = false;
        log::debug!(
            "[DEBUG-card] cards={cards} slot={slot} raw={text:?} match={reading:?} crop={}",
            if keep_crop {
                crop.display().to_string()
            } else {
                "-".to_owned()
            }
        );
        if !keep_crop {
            let _ = std::fs::remove_file(&crop);
        }
        // Blank means no text, not no match: with an empty pool an unmatched card has no match
        // either, and taking it for blank would let `read_cards_in` read a four-card screen's
        // middle two as a whole two-card screen. A read that named nothing because it was
        // ambiguous is a pool miss, and a pool miss is routine.
        match reading {
            CardReading::Named { name, score, .. } => read.push((name, score)),
            unnamed => return Err((slot, unnamed_reason(&unnamed))),
        }
    }
    Ok(read)
}

/// Crop one region and OCR it. Returns the text and the crop, which the caller deletes. It is
/// kept only when the read was poor enough to be worth looking at.
///
/// The card title is near-white text laid over arbitrary card art, and handing tesseract that
/// greyscale crop directly makes it read the art: a dark helmet behind a word garbles it, and card
/// borders at the edge of the crop come back as leading `|`, `Fr` or `pA UY`. Isolating the text
/// from the art is what fixes both, and it is a two-step job. `-normalize` first, so the cutoff is
/// relative to the crop's own brightness rather than an absolute grey level. That is what makes
/// one constant work across card art, and it should also absorb another machine's gamma. Then
/// `-threshold` to drop everything dimmer than the text, and `-negate` because tesseract is trained
/// on dark-on-light.
///
/// 74% is the middle of a plateau, not a tuned peak, and the plateau was re-swept against this Rust
/// pipeline rather than inherited from the ImageMagick one it replaced. Over the eleven labelled
/// fixture cards: 66% and below garbles a card, 70% and 74% read every card exactly, 78% drops
/// `Caliban Prime Chassis Blueprint` to 0.96 and 82% drops `Bronco Prime Receiver` to 0.89. So the
/// usable band is 70-78% and 74% sits in it with room on both sides.
fn read_region(
    image: &DynamicImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(String, PathBuf), &'static str> {
    let crop = scratch_file("reward-crop", "png");
    prepare_crop(image, x, y, width, height)
        .save(&crop)
        .map_err(|_| "could not write the reward card crop")?;
    let text = ocr_crop(&crop)?;
    Ok((text, crop))
}

/// Crop, greyscale, normalize, threshold, invert and upscale one card title.
///
/// Split from the file handling so the pipeline can be driven from pixels in a test rather than
/// only through a temp file.
pub fn prepare_crop(source: &DynamicImage, x: u32, y: u32, width: u32, height: u32) -> GrayImage {
    let cropped = source.view(x, y, width, height).to_image();
    let grey = cropped
        .pixels()
        .map(|pixel| luma(pixel[0], pixel[1], pixel[2]))
        .collect::<Vec<_>>();
    let prepared = threshold_inverted(&normalize_contrast(&grey));
    let prepared = GrayImage::from_raw(width, height, prepared)
        .unwrap_or_else(|| GrayImage::new(width, height));
    // 300%, matching what `magick -resize` did: tesseract reads a 58px title band poorly and a
    // 174px one exactly. ImageMagick's default filter is Mitchell, which `image` does not offer, so
    // the replacement was swept over the eleven labelled fixture cards rather than guessed at:
    // Nearest reads `Burston Prime Stock` at 0.89 and Triangle at 0.94, while CatmullRom and
    // Lanczos3 both read every card exactly bar the wrapped screen's known speck at 0.954. Either
    // of the last two would do; CatmullRom is the cheaper kernel.
    image::imageops::resize(
        &prepared,
        width * 3,
        height * 3,
        image::imageops::FilterType::CatmullRom,
    )
}

/// ImageMagick 7's `-colorspace gray`: a Rec.709 weighted sum of the *gamma-encoded* bytes.
///
/// `image`'s own `to_luma8` is Rec.601 and weights red at 76 rather than 54. On near-white text
/// over dark card art that difference is large enough to move pixels across the threshold, so the
/// weighting is spelled out here rather than taken from the crate.
pub fn luma(red: u8, green: u8, blue: u8) -> u8 {
    let value = 0.212_656 * red as f32 + 0.715_158 * green as f32 + 0.072_186 * blue as f32;
    value.round().clamp(0.0, 255.0) as u8
}

/// ImageMagick's `-normalize`, which is `-contrast-stretch 2%x1%` rather than a plain min-max
/// stretch.
///
/// The clipping is what makes one threshold constant work across card art: it discards the darkest
/// 2% and brightest 1% of the histogram before stretching, so a stray specular highlight cannot pin
/// the top of the range and leave the actual text sitting well below the cutoff.
pub fn normalize_contrast(grey: &[u8]) -> Vec<u8> {
    if grey.is_empty() {
        return Vec::new();
    }
    let mut histogram = [0_u32; 256];
    for value in grey {
        histogram[*value as usize] += 1;
    }
    let total = grey.len() as f64;
    let black_clip = (total * 0.02) as u32;
    let white_clip = (total * 0.01) as u32;

    let mut seen = 0;
    let low = histogram
        .iter()
        .position(|count| {
            seen += count;
            seen > black_clip
        })
        .unwrap_or(0) as u8;
    let mut seen = 0;
    let high = histogram
        .iter()
        .rposition(|count| {
            seen += count;
            seen > white_clip
        })
        .unwrap_or(255) as u8;

    // A flat crop (a capture taken a moment too early is entirely black) has no range to
    // stretch, and dividing by it would panic on exactly that frame.
    if high <= low {
        return grey.to_vec();
    }
    let span = (high - low) as f32;
    grey.iter()
        .map(|value| (((*value).clamp(low, high) - low) as f32 * 255.0 / span).round() as u8)
        .collect()
}

/// `-threshold 74% -negate` in one pass: keep what is brighter than the cutoff, then invert,
/// because tesseract is trained on dark-on-light.
pub fn threshold_inverted(grey: &[u8]) -> Vec<u8> {
    // ImageMagick compares strictly against the scaled cutoff: at 74% of 255 that is 188.7, so 188
    // is background and 189 is text.
    let cutoff = 0.74 * 255.0;
    grey.iter()
        .map(|value| if (*value as f32) > cutoff { 0 } else { 255 })
        .collect()
}

/// The bundled Tesseract's file name, which is the whole of the platform difference.
pub const TESSERACT_EXECUTABLE: &str = if cfg!(windows) {
    "tesseract.exe"
} else {
    "tesseract"
};

/// Which Tesseract to run, given the app's resource directory.
///
/// Windows has no package manager to lean on and no player should have to install an OCR engine
/// before the overlay works, so the NSIS bundle ships one under `tesseract/` and this prefers it.
/// The fallback is not a nicety: a `cargo test` run has no resource directory at all, and every
/// Linux package still gets Tesseract from the distribution.
pub fn tesseract_program(resource_dir: &Path) -> PathBuf {
    let bundled = resource_dir.join("tesseract").join(TESSERACT_EXECUTABLE);
    if bundled.is_file() {
        return bundled;
    }
    PathBuf::from("tesseract")
}

/// Set once at startup from the resolved resource directory, because the OCR path is reached from
/// worker threads that have no `AppHandle` to ask.
static TESSERACT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Point the OCR path at the bundled engine. Called once from Tauri's `setup`; later calls lose,
/// which is what makes this safe to call from a test that only wants the default.
pub fn use_bundled_tesseract(resource_dir: &Path) {
    let _ = TESSERACT.set(tesseract_program(resource_dir));
}

/// OCR a crop that `read_region` has already isolated to text.
///
/// `--psm 11`, sparse text, rather than the obvious `--psm 6`, one uniform block. The title band
/// reserves room above the title for a second line, and on a one-line title that room is empty,
/// so anything the game draws up there arrives as a speck floating above the words. `psm 6` has to
/// call one of them "the block", and when it picks the speck it does not merely add noise, it
/// returns the speck *instead of the title*: a real 2026-07-28 crop reading `Dual Zoren Prime
/// Handle` came back as `"| @\nn |\n|"`. Every poll failed that way until the speck went, which cost
/// about nine seconds of a fifteen-second screen.
///
/// `psm 11` does not have to choose, since it reads every text region it finds. Swept over the twelve
/// labelled crops from four captured screens plus that live one, `psm 11` and `psm 12` read all
/// twelve; `psm 3`, `4` and `6` miss the speck case entirely, `psm 7` mangles wrapped titles, and
/// `psm 13` clips leading letters. `11` over `12` only because `12` adds orientation detection this
/// does not need. What it costs is a little leading punctuation, which `normalise` drops before the
/// match ever sees it.
pub fn ocr_crop(image: &Path) -> Result<String, &'static str> {
    run_tesseract(image, "11", None)
}

/// Kiosk label slots: `--psm 6`, one uniform block of text.
///
/// The deliberate counterpart to [`ocr_crop`]'s `psm 11`, not a contradiction of it. Reward
/// title bands are mostly empty room with a possible speck, and `psm 6` calls the speck the
/// block. Kiosk label crops are the inverse: the label fills the bottom of the crop and the
/// card art fills the top, and bright art is a large white mass, not a speck. Under `psm 11`
/// that mass joins the segmentation and eats any glyphs touching it; `psm 6` makes the
/// narrower claim (one ordered block) for a one-label crop.
pub fn ocr_crop_block(image: &Path) -> Result<String, &'static str> {
    run_tesseract(image, "6", None)
}

fn run_tesseract(
    image: &Path,
    page_segmentation_mode: &str,
    whitelist: Option<&str>,
) -> Result<String, &'static str> {
    let program = TESSERACT
        .get()
        .cloned()
        .unwrap_or_else(|| "tesseract".into());
    let mut command = Command::new(&program);
    // A GUI parent otherwise opens a console for this console-subsystem child.
    // Reward polling can spawn three crops in quick succession (issue #12).
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    // One recognition thread per spawn: tesseract's own OpenMP pooling oversubscribes the
    // machine when several crops are read side by side, and the kiosk poller does exactly that.
    command.env("OMP_THREAD_LIMIT", "1");
    // The bundled engine's `eng.traineddata` sits beside it, not in the install prefix it was
    // compiled with, so it has to be told where to look. `--tessdata-dir` rather than the
    // `TESSDATA_PREFIX` environment variable because setting one of those is `unsafe` since the
    // 2024 edition, and this crate forbids that.
    if let Some(directory) = program.parent().filter(|path| !path.as_os_str().is_empty()) {
        command.args([
            std::ffi::OsStr::new("--tessdata-dir"),
            tessdata_dir_arg(directory).as_os_str(),
        ]);
    }
    command
        .arg(image)
        .args(["-", "--psm", page_segmentation_mode]);
    if let Some(whitelist) = whitelist {
        command.args(["-c", &format!("tessedit_char_whitelist={whitelist}")]);
    }
    let text = command.output().map_err(|_| "tesseract is not available")?;
    Ok(String::from_utf8_lossy(&text.stdout).into_owned())
}

/// The `--tessdata-dir` value Tesseract can actually open.
///
/// The installed app resolves its resource directory in verbatim `\\?\` form (issue #12),
/// but Tesseract joins this directory to `eng.traineddata` with a forward slash, which
/// verbatim paths reject, so the open fails even though the file shipped. Strip the
/// prefix; ordinary paths are untouched. String-level rather than `Prefix`-based so the
/// behaviour is testable on every platform: install paths are valid Unicode in practice.
fn tessdata_dir_arg(directory: &Path) -> PathBuf {
    let text = directory.as_os_str().to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    directory.to_path_buf()
}

/// Compare on alphanumerics only. That is what lets a read of "2 X Forma Blueprint W\:" land on
/// "2X Forma Blueprint" instead of drifting to a different reward.
fn normalise(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

/// How near-exact a read must be to speak for a whole card, as percentage integers rather than
/// scores. Admission in the matcher is [`within_band`] on an exact edit count, so no float
/// rounding can move a card across a threshold; `1.0 - edits/span` is computed only to be
/// reported, to the log and to the UI's confidence.
///
/// No f32 decides admission anywhere in this crate. The float scores exist to be reported, and
/// every threshold that names or refuses a reward is an integer edit budget.
///
/// The trusted band is what lets a fragment of a read name a card at all. Scoring fragments is
/// how a card is found under the specks Tesseract returns at `--psm 11`, but it cannot be done at
/// the pool's band: `blueprint` is a suffix of most rewards and alone scores 0.64 against `Forma
/// Blueprint`, so a mis-crop that recovers one generic word would resolve to a confident wrong
/// reward. Measured: `WOH DIGeil / Blueprint`, this file's own 2026-08-20 wrong-monitor read,
/// scored 0.56 and was rejected before fragments were scored, and 0.64 and accepted after. A
/// fragment has to be a near-exact match to speak.
const TRUSTED_EDITS_PERCENT: usize = 15;

/// How near-exact a read of a pool reward must be for the pool's nearest name to name it.
///
/// The pool is a couple of dozen rewards, so a garbled read still has one clear nearest name.
/// This is the band that decides whether the pool may name a card at all: 40% of the span in
/// edits, which is the old 0.6 score floor expressed as an exact edit count.
const POOL_EDITS_PERCENT: usize = 40;

/// How near-exact a read must be to name a reward from outside the squad's pool.
///
/// The pool is a couple of dozen rewards, so a garbled read still has one clear nearest name.
/// Outside it there are hundreds of near neighbours, every part of every Prime, so only a
/// near-exact read may name one.
const CATALOG_EDITS_PERCENT: usize = 10;

/// Whether `edits` sits within `percent` of `span`. Integer multiplication, so the answer is
/// exact whatever the score rounds to.
fn within_band(edits: usize, span: usize, percent: usize) -> bool {
    edits * 100 <= span * percent
}

/// The card's text split on blank lines, each group's own lines rejoined with a space.
///
/// Blank means whitespace-only rather than exactly `"\n\n"`: tesseract's stdout is passed through
/// `String::from_utf8_lossy` with no newline normalisation, so on Windows the separator is
/// `"\r\n\r\n"`, and a blank line that carries a stray space is common on a dark card. Splitting
/// on the literal `"\n\n"` left both shapes as a single group, which made this whole mechanism
/// silently inert exactly where it was most needed.
fn text_groups(text: &str) -> Vec<String> {
    let mut groups = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !current.is_empty() {
                groups.push(current.join(" "));
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        groups.push(current.join(" "));
    }
    groups.retain(|group| !normalise(group).is_empty());
    groups
}

/// Where a named card came from: the squad's relic pool, or the reward catalogue behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchSource {
    Pool,
    Catalog,
}

/// The nearest candidate to a piece of a card's text, and how far it was.
///
/// `edits` and `span` are exact counts, and they are what every admission decision is made on.
struct Nearest {
    name: String,
    /// The text this candidate was scored against, which is a fragment of the card whenever a
    /// fragment won. It is the only valid thing to measure a rival against: comparing a whole
    /// read to a fragment's name would charge the rival for text the winner never had to match.
    read: String,
    edits: usize,
    /// The longer of the two names' lengths, and never zero, so the reported score cannot divide
    /// by zero on a card whose text and candidates are both empty.
    span: usize,
    /// Which list this name was taken from. The pool and the catalogue have different bands, and
    /// a catalogue name folded onto the pool's spelling is still a pool answer.
    source: MatchSource,
}

impl Nearest {
    /// The score the log and the UI's confidence see. Reported, never used to decide anything.
    fn score(&self) -> f32 {
        Self::score_of(self.edits, self.span)
    }

    fn score_of(edits: usize, span: usize) -> f32 {
        1.0 - edits as f32 / span as f32
    }

    fn within(&self, percent: usize) -> bool {
        within_band(self.edits, self.span, percent)
    }

    /// The band that decides whether this answer may name a card.
    fn band_percent(&self) -> usize {
        match self.source {
            MatchSource::Pool => POOL_EDITS_PERCENT,
            MatchSource::Catalog => CATALOG_EDITS_PERCENT,
        }
    }

    fn pair(self) -> (String, f32) {
        let score = self.score();
        (self.name, score)
    }
}

/// The nearest of `candidates` to `text`, or none when the text has nothing to compare.
fn nearest(text: &str, candidates: &[RewardCatalogEntry], source: MatchSource) -> Option<Nearest> {
    let read = normalise(text);
    if read.is_empty() {
        return None;
    }
    let read_length = read.chars().count();
    candidates
        .iter()
        .map(|candidate| {
            let known = normalise(&candidate.name);
            let edits = edit_distance(&read, &known);
            let span = read_length.max(known.chars().count()).max(1);
            (candidate, edits, span)
        })
        .max_by(|(_, left, left_span), (_, right, right_span)| {
            Nearest::score_of(*left, *left_span).total_cmp(&Nearest::score_of(*right, *right_span))
        })
        .map(|(candidate, edits, span)| Nearest {
            name: candidate.name.clone(),
            read,
            edits,
            span,
            source,
        })
}

/// The nearest of the pool to a card, scoring the whole read and then its fragments.
///
/// Two bands, because the two kinds of read carry different weight. The whole text is what the
/// card actually said, so it is taken as it comes; the pool's band is applied to the result by the
/// caller. The whole text resolves a wrapped name, where `Dual Zoren Prime` and `Blueprint` each
/// match nothing alone. A fragment is a guess about which part of the read is the name, so it only
/// counts at the trusted band: that is what recovers `Forma Blueprint` from under three specks of
/// noise (0.64 -> 1.0) without letting a lone `Blueprint` name a card it cannot identify.
///
/// Still a closed-set match: it returns the nearest of `candidates`, never "none of these", so the
/// bands are the only guard against a confident wrong answer. Which names a card may reach at
/// all is [`choose`]'s call.
fn pool_nearest(text: &str, candidates: &[RewardCatalogEntry]) -> Option<Nearest> {
    let whole = nearest(text, candidates, MatchSource::Pool);
    let groups = text_groups(text);
    let mut fragments = groups.clone();
    for start in 0..groups.len() {
        fragments.push(groups[start..].join(" "));
    }
    // Deduped on the normalised form, because `normalise` strips the separators that are the only
    // difference between a wrapped name and its rejoined groups. On raw text every such pair
    // scores twice. Seeding with the whole text drops the `start == 0` run, which normalises to
    // exactly it. That is to avoid a redundant pass over the pool, not to protect a band: the
    // whole text is scored above without a local band either way, and a duplicate of it can only
    // tie that score, never change the result whose score the caller checks against the pool's.
    let mut seen = vec![normalise(text)];
    let mut distinct = Vec::with_capacity(fragments.len());
    for fragment in fragments {
        let normalised = normalise(&fragment);
        if normalised.is_empty() || seen.contains(&normalised) {
            continue;
        }
        seen.push(normalised);
        distinct.push(fragment);
    }
    let best_fragment = distinct
        .iter()
        .filter_map(|fragment| nearest(fragment, candidates, MatchSource::Pool))
        .filter(|fragment| fragment.within(TRUSTED_EDITS_PERCENT))
        .max_by(|left, right| left.score().total_cmp(&right.score()));
    match (whole, best_fragment) {
        (Some(whole), Some(fragment)) => Some(if fragment.score() > whole.score() {
            fragment
        } else {
            whole
        }),
        (whole, fragment) => whole.or(fragment),
    }
}

/// The card's best match against the squad's pool, falling back to the reward catalogue.
///
/// A pool read inside the trusted band stands untouched, so `Forma Blueprint` still names the
/// pool's `2X Forma Blueprint` rather than the catalogue's `Forma Blueprint`. Below that the
/// catalogue's answer counts only inside the catalogue's own, tighter band, so it can name a card
/// the pool is missing but never redirect a fuzzy read. A relic WFCD lists with no rewards left
/// `Baza Prime Blueprint` out of a live pool, and its clean read was published as `Burston Prime
/// Blueprint`.
///
/// The catalogue is scored on the card's whole text, never on a fragment of it. It lists a
/// Warframe part's component beside its blueprint, so a wrapped `Lavos Prime Chassis Blueprint`
/// whose second line read badly would otherwise be named `Lavos Prime Chassis` from its first.
/// A catalogue answer the pool also lists under the relic table's spelling takes that spelling:
/// a garbled `Forma Blueprint` read still names what the relic drops, `2X Forma Blueprint`.
fn choose(text: &str, candidates: CardCandidates<'_>) -> Option<Nearest> {
    let pool = pool_nearest(text, candidates.pool);
    if pool
        .as_ref()
        .is_some_and(|nearest| nearest.within(TRUSTED_EDITS_PERCENT))
    {
        return pool;
    }
    let catalog = nearest(text, candidates.catalog, MatchSource::Catalog)
        .filter(|nearest| nearest.within(CATALOG_EDITS_PERCENT))
        .map(|nearest| {
            let spelling = candidates.pool.iter().find(|entry| {
                warframe_acquisition::reward_name_matches(&nearest.name, &entry.name)
            });
            match spelling {
                Some(entry) => Nearest {
                    name: entry.name.clone(),
                    // The pool's spelling is what names the card, so it is a pool answer and is
                    // judged at the pool's band. The catalogue still supplied the match.
                    source: MatchSource::Pool,
                    ..nearest
                },
                None => nearest,
            }
        });
    match (pool, catalog) {
        (Some(pool), Some(catalog)) => Some(if catalog.score() > pool.score() {
            catalog
        } else {
            pool
        }),
        (pool, catalog) => pool.or(catalog),
    }
}

/// Why a card's read named no reward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownReason {
    /// No candidate was close enough to name.
    BelowFloor,
    /// A different reward sits within one edit of the read that named the winner, so the read
    /// cannot say which of the two it saw.
    Ambiguous,
}

/// What a card's read resolved to.
#[derive(Clone, Debug, PartialEq)]
pub enum CardReading {
    /// The read named a reward, at the score it matched at.
    Named {
        name: String,
        score: f32,
        /// The exact edit count behind the score. Reported, never a threshold.
        edits: usize,
        source: MatchSource,
    },
    /// The read held no text at all.
    Blank,
    /// The read held text but named nothing.
    Unknown(UnknownReason),
}

/// Why a card that named nothing failed its slot.
///
/// Both unknown reasons carry the same string: the card's text did not identify a reward. The
/// recognition layer treats the pool miss as routine and any other reason as a capture failure,
/// so an ambiguous screen must not warn as though the capture were broken.
fn unnamed_reason(reading: &CardReading) -> &'static str {
    match reading {
        CardReading::Unknown(_) => POOL_MISS,
        CardReading::Blank => BLANK_CARD,
        // A named card never reaches here; the caller only asks about the ones that did not
        // name. The arm exists because the match has to be exhaustive.
        CardReading::Named { .. } => POOL_MISS,
    }
}

/// Whether another reward sits close enough to the winner that the read could have meant it.
///
/// Two conditions, and both are needed. The rival's name must sit within the winner's own edit
/// budget plus one, because a name further out than that is not what makes the two rewards hard to
/// tell apart. And the rival must itself be a plausible reading of the same text, at the trusted
/// band, because otherwise the name distance is measuring the noise in the read rather than the
/// reward.
///
/// The second condition is what keeps a noisy read of one reward from refusing to name it. The
/// winner's own edit count grows with the noise in the read, and so does the budget this compares
/// against, so a read carrying four stray characters would otherwise be within one edit of a
/// reward the read plainly does not contain. The kiosk's labels are read with exactly that kind
/// of noise, and a name the read plainly holds was being dropped for it.
///
/// The rival is measured against the winner's own scoring text, which is the fragment that won
/// when a fragment won. Measuring against the whole read would charge the rival for text the
/// winner never had to match.
///
/// A reward the pool and the catalogue spell differently is the same reward wearing two names,
/// not a rival, so the names are compared through the same fold [`choose`] uses.
fn rival_within_one_edit(winner: &Nearest, candidates: CardCandidates<'_>) -> bool {
    let budget = winner.edits + 1;
    let winner_name = normalise(&winner.name);
    candidates
        .pool
        .iter()
        .chain(candidates.catalog.iter())
        .any(|entry| {
            if same_reward(&winner.name, &entry.name) {
                return false;
            }
            let rival = normalise(&entry.name);
            if edit_distance(&winner_name, &rival) > budget {
                return false;
            }
            let span = winner.read.chars().count().max(rival.chars().count());
            within_band(
                edit_distance(&winner.read, &rival),
                span,
                TRUSTED_EDITS_PERCENT,
            )
        })
}

/// Whether two names are the same reward under either spelling.
fn same_reward(left: &str, right: &str) -> bool {
    warframe_acquisition::reward_name_matches(left, right)
        || warframe_acquisition::reward_name_matches(right, left)
}

/// What a card's read resolves to: a named reward, a blank, or a reason it named nothing.
///
/// The scorer behind this answers with the nearest name whatever the read looks like, which is
/// what makes it useful and what makes it unsafe on its own: a read one edit from one reward and
/// two from its sibling scores well against the first and is published as it. This is where that
/// becomes a decision instead of a score.
///
/// The order is deliberate. A blank read is a blank card, not a pool miss, and the two are not the
/// same thing to the caller: a blank first slot means the block is narrower and a narrower layout
/// may be tried, while a pool miss means the text was there and the pool did not have it.
///
/// Admission is an integer edit budget throughout. The float score is computed only to be
/// reported, to the log and to the UI's confidence.
pub fn resolve_card(text: &str, candidates: CardCandidates<'_>) -> CardReading {
    if normalise(text).is_empty() {
        return CardReading::Blank;
    }
    let Some(winner) = choose(text, candidates) else {
        return CardReading::Unknown(UnknownReason::BelowFloor);
    };
    if !winner.within(winner.band_percent()) {
        return CardReading::Unknown(UnknownReason::BelowFloor);
    }
    let score = winner.score();
    let edits = winner.edits;
    let source = winner.source;
    if edits == 0 {
        return CardReading::Named {
            name: winner.name,
            score,
            edits,
            source,
        };
    }
    if rival_within_one_edit(&winner, candidates) {
        return CardReading::Unknown(UnknownReason::Ambiguous);
    }
    CardReading::Named {
        name: winner.name,
        score,
        edits,
        source,
    }
}

/// The best match for a card against the squad's pool, falling back to the reward catalogue.
/// See [`choose`] for what wins, and why.
pub fn best_card_match(text: &str, candidates: CardCandidates<'_>) -> Option<(String, f32)> {
    choose(text, candidates).map(Nearest::pair)
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];
    for (row, left_char) in left.chars().enumerate() {
        current[0] = row + 1;
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != *right_char);
            current[column + 1] = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::overlay_window::WindowRect;
    use crate::reward_capture::{CapturedFrame, FrameBackend, RectOrigin};

    use super::{clear_matched_rect, read_capture_candidates};

    /// A flat monitor capture, tagged so a frame can be traced back to the screen it came from.
    fn monitor(width: u32, height: u32, tag: u8) -> image::RgbaImage {
        image::RgbaImage::from_pixel(width, height, image::Rgba([tag, tag, tag, 255]))
    }

    fn candidate(x: i32, tag: u8) -> CapturedFrame {
        CapturedFrame {
            rect: WindowRect {
                x,
                y: 0,
                width: 1920,
                height: 1080,
            },
            image: image::DynamicImage::ImageRgba8(monitor(1, 1, tag)),
            rect_origin: RectOrigin::X11,
            frame_backend: FrameBackend::X11,
        }
    }

    /// Mutation caught: publishing before OCR or returning the first failure would leave the
    /// overlay on monitor A even though only monitor B contained the reward screen.
    #[test]
    fn failed_candidate_a_then_successful_b_publishes_b() {
        let matched = Mutex::new(None);
        let frames = [candidate(0, 10), candidate(1920, 20)];

        let result = read_capture_candidates(&frames, &matched, |image| {
            if image.to_rgba8().get_pixel(0, 0)[0] == 20 {
                Ok(vec!["Forma Blueprint".to_owned()])
            } else {
                Err("reward card text did not match the relic pool")
            }
        });

        assert_eq!(result, Ok(vec!["Forma Blueprint".to_owned()]));
        assert_eq!(*matched.lock().unwrap(), Some(frames[1].rect));
    }

    /// Mutation caught: writing each attempted rect would publish the final failed monitor even
    /// though no candidate had proven it contained Warframe's reward screen.
    #[test]
    fn all_failed_candidates_publish_nothing_and_keep_the_last_reason() {
        let matched = Mutex::new(None);
        let frames = [candidate(0, 10), candidate(1920, 20)];

        let result: Result<(), &'static str> =
            read_capture_candidates(&frames, &matched, |image| {
                if image.to_rgba8().get_pixel(0, 0)[0] == 10 {
                    Err("a reward card read as blank")
                } else {
                    Err("reward card text did not match the relic pool")
                }
            });

        assert_eq!(result, Err("reward card text did not match the relic pool"));
        assert_eq!(*matched.lock().unwrap(), None);
    }

    /// Mutation caught: `lock().ok()` would silently stop publishing forever after one panic
    /// poisoned the process-wide matched-rect snapshot.
    #[test]
    fn matched_rect_publication_recovers_a_poisoned_snapshot() {
        let matched = Mutex::new(None);
        let _ = std::panic::catch_unwind(|| {
            let _guard = matched.lock().unwrap();
            panic!("poison the local matched-rect snapshot");
        });
        let frame = candidate(1920, 20);

        let result = read_capture_candidates(std::slice::from_ref(&frame), &matched, |_| Ok(()));

        assert_eq!(result, Ok(()));
        assert_eq!(
            *matched
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            Some(frame.rect)
        );
    }

    /// Mutation caught: game-exit teardown hid the overlay but retained the previous session's
    /// monitor rectangle, so previews could be positioned against stale geometry.
    #[test]
    fn clearing_matched_rect_removes_the_previous_session() {
        let matched = Mutex::new(Some(candidate(1920, 20).rect));
        clear_matched_rect(&matched);
        assert_eq!(*matched.lock().unwrap(), None);
    }

    #[test]
    fn clearing_matched_rect_recovers_a_poisoned_snapshot() {
        let matched = Mutex::new(Some(candidate(1920, 20).rect));
        let _ = std::panic::catch_unwind(|| {
            let _guard = matched.lock().unwrap();
            panic!("poison the local matched-rect snapshot");
        });

        clear_matched_rect(&matched);

        assert_eq!(
            *matched
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            None
        );
    }

    fn pool_entry(name: &str) -> super::RewardCatalogEntry {
        super::RewardCatalogEntry {
            name: name.to_owned(),
            ducats: 0,
        }
    }

    /// The relic pool alone, with no catalogue behind it: what every read had before a card
    /// could be named from outside the pool.
    fn pool_only(pool: &[super::RewardCatalogEntry]) -> super::CardCandidates<'_> {
        super::CardCandidates { pool, catalog: &[] }
    }

    /// A read one edit from one reward and two from its sibling has not named either, however
    /// good the nearest-name score looks. The scorer still answers; the resolver is where the
    /// decision is made, so a wrong name is never published as a trusted one.
    #[test]
    fn an_ambiguous_read_names_nothing() {
        let catalog = [
            pool_entry("Gara Prime Neuroptics Blueprint"),
            pool_entry("Garuda Prime Neuroptics Blueprint"),
        ];
        let read = "Gara Prime Neuroptics Blueprlnt";
        assert_eq!(
            super::resolve_card(
                read,
                super::CardCandidates {
                    pool: &[],
                    catalog: &catalog,
                },
            ),
            super::CardReading::Unknown(super::UnknownReason::Ambiguous),
        );
        let (name, score) = super::best_card_match(
            read,
            super::CardCandidates {
                pool: &[],
                catalog: &catalog,
            },
        )
        .expect("the scorer still names its nearest");
        assert_eq!(name, "Gara Prime Neuroptics Blueprint");
        assert!(score >= 0.9, "scored {score}");
    }

    /// An exact read is not ambiguous, however close its sibling is.
    #[test]
    fn an_exact_read_names_its_card_even_beside_a_sibling() {
        let catalog = [
            pool_entry("Gara Prime Neuroptics Blueprint"),
            pool_entry("Gara Prime Neuroptic Blueprint"),
        ];
        assert_eq!(
            super::resolve_card(
                "Gara Prime Neuroptics Blueprint",
                super::CardCandidates {
                    pool: &[],
                    catalog: &catalog,
                },
            ),
            super::CardReading::Named {
                name: "Gara Prime Neuroptics Blueprint".to_owned(),
                score: 1.0,
                edits: 0,
                source: super::MatchSource::Catalog,
            },
        );
    }

    /// A sibling three edits from the winner's name cannot be what the read said, so it does not
    /// block the name. The read is two edits from the sibling, inside the trusted band, so the
    /// name-distance budget is the only thing that rejects it.
    #[test]
    fn a_rival_three_edits_away_does_not_block_the_name() {
        let pool = [pool_entry("Paris Prime Lower Limb")];
        let catalog = [pool_entry("Paris Prime Upper Limb")];
        assert_eq!(
            super::resolve_card(
                "Paris Prime Uower Limb",
                super::CardCandidates {
                    pool: &pool,
                    catalog: &catalog,
                },
            ),
            super::CardReading::Named {
                name: "Paris Prime Lower Limb".to_owned(),
                score: 1.0 - 1.0 / 19.0,
                edits: 1,
                source: super::MatchSource::Pool,
            },
        );
    }

    /// The pool spells a stack's reward `2X Forma Blueprint` and the catalogue spells it
    /// `Forma Blueprint`. Those are one reward wearing two names, not two rewards, so the
    /// spelling difference must not read as ambiguity.
    #[test]
    fn a_catalogue_name_the_pool_spells_differently_is_not_a_rival() {
        let pool = [
            pool_entry("Kestrel Prime Grip"),
            pool_entry("Alternox Prime Barrel"),
            pool_entry("2X Forma Blueprint"),
            pool_entry("Xaku Prime Neuroptics Blueprint"),
            pool_entry("Burston Prime Blueprint"),
            pool_entry("Lex Prime Barrel"),
        ];
        let mut catalog = pool.to_vec();
        catalog.extend([
            pool_entry("Baza Prime Blueprint"),
            pool_entry("Burston Prime Barrel"),
            pool_entry("Burston Prime Receiver"),
            pool_entry("Burston Prime Stock"),
            pool_entry("Braton Prime Blueprint"),
            pool_entry("Braton Prime Barrel"),
            pool_entry("Braton Prime Receiver"),
            pool_entry("Braton Prime Stock"),
            pool_entry("Forma Blueprint"),
        ]);
        assert_eq!(
            super::resolve_card(
                "Forma Bluepr1nt",
                super::CardCandidates {
                    pool: &pool,
                    catalog: &catalog,
                },
            ),
            super::CardReading::Named {
                name: "2X Forma Blueprint".to_owned(),
                score: 1.0 - 1.0 / 14.0,
                edits: 1,
                source: super::MatchSource::Pool,
            },
        );
    }

    /// The pool band's edge is inclusive: a read exactly 40% of its span away from the only pool
    /// name still names it, which is the old 0.6 score floor expressed as an edit budget.
    #[test]
    fn a_read_exactly_on_the_pool_band_names_its_card() {
        let name = "Athodai Prime Receiver";
        assert_eq!(super::normalise(name).chars().count(), 20);
        let pool = [pool_entry(name)];
        assert_eq!(
            super::resolve_card("qqqqqqq Qrime Receiver", pool_only(&pool)),
            super::CardReading::Named {
                name: name.to_owned(),
                score: 1.0 - 8.0 / 20.0,
                edits: 8,
                source: super::MatchSource::Pool,
            },
        );
        assert_eq!(
            super::resolve_card("qqqqqqq Qqime Receiver", pool_only(&pool)),
            super::CardReading::Unknown(super::UnknownReason::BelowFloor),
            "a read past the pool band names nothing"
        );
    }

    /// A fragment is a guess at which part of the read is the name, so its band is tighter than
    /// the whole read's. The edge is inclusive: three edits of a twenty-character name.
    #[test]
    fn a_fragment_exactly_on_the_trusted_band_names_the_card() {
        let name = "Athodai Prime Receiver";
        let pool = [pool_entry(name)];
        assert_eq!(
            super::resolve_card("WOH DIGeilzzzz\n\nAthodoi Qrime Reqeiver", pool_only(&pool)),
            super::CardReading::Named {
                name: name.to_owned(),
                score: 1.0 - 3.0 / 20.0,
                edits: 3,
                source: super::MatchSource::Pool,
            },
        );
    }

    /// A fragment four edits away over a span of twenty-four is 16.7%: inside the pool's band and
    /// outside the trusted band, so it must not name the card. The whole read around it is
    /// farther still.
    #[test]
    fn a_fragment_past_the_trusted_band_names_nothing() {
        let name = "Athodai Prime Receiver";
        let pool = [pool_entry(name)];
        assert_eq!(
            super::resolve_card(
                "WOH DIGeilzzz\n\nAthodai Prime Receiverzzzz",
                pool_only(&pool)
            ),
            super::CardReading::Unknown(super::UnknownReason::BelowFloor),
        );
    }

    /// A blank read and a read that matched nothing are different answers. The first means the
    /// crop was narrower than the card; the second means the card was not in the pool, and only
    /// the first may let a narrower layout be tried.
    #[test]
    fn a_blank_read_is_not_a_pool_miss() {
        let pool = [
            pool_entry("Braton Prime Blueprint"),
            pool_entry("2X Forma Blueprint"),
            pool_entry("Burston Prime Stock"),
            pool_entry("Trumna Prime Blueprint"),
        ];
        let blank = super::resolve_card("\n\n   ", pool_only(&pool));
        let miss = super::resolve_card("Orokin Catalyst Blueprint", pool_only(&pool));
        assert_eq!(blank, super::CardReading::Blank);
        assert_eq!(
            miss,
            super::CardReading::Unknown(super::UnknownReason::BelowFloor)
        );
        assert_ne!(
            super::unnamed_reason(&blank),
            super::unnamed_reason(&miss),
            "a blank first slot must be told apart from a pool miss, or the narrower layout is never tried"
        );
    }

    /// An ambiguous card is a read that did not identify a reward, not a broken capture. The
    /// recognition layer warns on any reason but the pool miss and the blank, so an ambiguous
    /// screen must carry the pool miss's own string.
    #[test]
    fn an_ambiguous_card_is_a_routine_miss_not_a_capture_failure() {
        assert_eq!(
            super::unnamed_reason(&super::CardReading::Unknown(
                super::UnknownReason::Ambiguous
            )),
            "reward card text did not match the relic pool",
        );
        assert_eq!(
            super::unnamed_reason(&super::CardReading::Unknown(
                super::UnknownReason::BelowFloor
            )),
            "reward card text did not match the relic pool",
        );
    }

    /// The 2026-08-22 report's own slot 1: Tesseract returns three noise fragments above the
    /// real name. Scoring the whole blob puts this at 0.636 against a 0.6 floor, so one more
    /// speck of noise would have discarded a correct read.
    #[test]
    fn a_card_read_with_noise_above_it_scores_on_its_own_line() {
        let pool = [
            pool_entry("Forma Blueprint"),
            pool_entry("Wisp Prime Neuroptics Blueprint"),
            pool_entry("Dual Zoren Prime Blueprint"),
        ];
        let (name, score) =
            super::best_card_match("&\n\nvr\n\ni STrTl\n\nForma Blueprint", pool_only(&pool))
                .expect("a noisy read still resolves");
        assert_eq!(name, "Forma Blueprint");
        assert!(
            score > 0.9,
            "scored {score}: the noise is still being scored against the pool"
        );
    }

    /// The reason the whole-text candidate has to stay: the game wraps a long reward name onto
    /// two lines, and each line alone matches nothing.
    #[test]
    fn a_wrapped_reward_name_still_matches_across_its_lines() {
        let pool = [
            pool_entry("Dual Zoren Prime Blueprint"),
            pool_entry("Forma Blueprint"),
        ];
        let (name, score) =
            super::best_card_match("Dual Zoren Prime\n\nBlueprint", pool_only(&pool))
                .expect("a wrapped name resolves");
        assert_eq!(name, "Dual Zoren Prime Blueprint");
        assert!(score > 0.99, "scored {score} on an exact wrapped read");
    }

    /// Scoring more candidates must not invent a match out of pure noise: the band is the only
    /// thing standing between a garbled read and a confident wrong answer.
    #[test]
    fn pure_noise_still_scores_below_the_match_floor() {
        let pool = [
            pool_entry("Forma Blueprint"),
            pool_entry("Wisp Prime Neuroptics Blueprint"),
        ];
        for noise in ["&\n\nvr\n\ni STrTl", "xx\n\nzzz qq"] {
            assert!(
                matches!(
                    super::resolve_card(noise, pool_only(&pool)),
                    super::CardReading::Unknown(_)
                ),
                "{noise:?} named a card out of pure noise"
            );
        }

        // Asserted as an absence rather than a low score: a score of 0.0 from `unwrap_or_default`
        // is below the floor whatever the implementation does, so it would pass vacuously.
        assert!(
            super::best_card_match("\n\n\n", pool_only(&pool)).is_none(),
            "a read with no alphanumerics has nothing to match and must be an absence"
        );
    }

    /// A mis-crop that recovers one generic word must not name a card. `Blueprint` is a suffix on
    /// most Warframe rewards, so alone it identifies nothing, yet it scores 0.64 against
    /// `Forma Blueprint`, over the pool band.
    ///
    /// `WOH DIGeil` is this file's own recorded read of the 2026-08-20 wrong-monitor capture. That
    /// bug surfaced as `reward card text did not match the relic pool`; scoring fragments at the
    /// pool band would have turned it into a confident wrong reward instead.
    #[test]
    fn a_mis_cropped_read_that_recovers_one_generic_word_is_rejected() {
        let pool = [
            pool_entry("Forma Blueprint"),
            pool_entry("Wisp Prime Neuroptics Blueprint"),
            pool_entry("Dual Zoren Prime Blueprint"),
        ];
        for mis_crop in ["WOH DIGeil\n\nBlueprint", "aaaa bbbb\n\ncccc\n\nBlueprint"] {
            assert!(
                matches!(
                    super::resolve_card(mis_crop, pool_only(&pool)),
                    super::CardReading::Unknown(_)
                ),
                "{mis_crop:?} named a card: one generic word is naming a card it cannot identify"
            );
        }
    }

    /// `ocr_crop` hands back tesseract's stdout through `String::from_utf8_lossy` with no newline
    /// normalisation, and Windows is a supported capture path, so grouping that keys on the
    /// literal `"\n\n"` is inert on exactly the platform half of the users are on.
    #[test]
    fn noise_above_a_name_is_read_through_a_windows_line_ending() {
        let pool = [
            pool_entry("Forma Blueprint"),
            pool_entry("Wisp Prime Neuroptics Blueprint"),
        ];
        let (name, score) = super::best_card_match(
            "&\r\n\r\nvr\r\n\r\ni STrTl\r\n\r\nForma Blueprint",
            pool_only(&pool),
        )
        .expect("a CRLF read still resolves");
        assert_eq!(name, "Forma Blueprint");
        assert!(score > 0.9, "scored {score} on a CRLF read");
    }

    /// A "blank" line off a dark card routinely carries a stray space, which is not an empty
    /// string. Treating blank as whitespace-only is what keeps the groups separated.
    #[test]
    fn a_blank_line_carrying_a_space_still_separates_groups() {
        let pool = [
            pool_entry("Forma Blueprint"),
            pool_entry("Wisp Prime Neuroptics Blueprint"),
        ];
        let (name, score) =
            super::best_card_match("&\n \nvr\n \ni STrTl\n \nForma Blueprint", pool_only(&pool))
                .expect("a read whose blank lines carry spaces still resolves");
        assert_eq!(name, "Forma Blueprint");
        assert!(score > 0.9, "scored {score} on a space-separated read");
    }

    /// The case between the other two: noise above a name that is itself wrapped. Neither the
    /// whole text nor any single group matches here: only a trailing run of groups does, which
    /// is what makes that loop load-bearing rather than decoration.
    #[test]
    fn a_trailing_run_of_groups_recovers_a_wrapped_name_under_noise() {
        let pool = [
            pool_entry("Dual Zoren Prime Blueprint"),
            pool_entry("Forma Blueprint"),
            pool_entry("Wisp Prime Neuroptics Blueprint"),
        ];
        let (name, score) =
            super::best_card_match("vr\n\nDual Zoren Prime\n\nBlueprint", pool_only(&pool))
                .expect("a wrapped name under noise resolves");
        assert_eq!(name, "Dual Zoren Prime Blueprint");
        // Asserted near-exact, not merely over the floor: the whole-text read already carries this
        // input to 0.92, because two chars of noise against a 25-char read is a small penalty. So
        // a `> 0.9` assertion passes with the trailing-run loop deleted and pins nothing. Only the
        // rejoined run `Dual Zoren Prime Blueprint` reaches 1.0.
        assert!(score > 0.99, "scored {score} on a wrapped name under noise");
    }

    /// Issue #12: the installed app resolves its resource directory in verbatim `\\?\` form,
    /// and Tesseract joins `--tessdata-dir` to the filename with `/`, which verbatim paths
    /// reject, so `eng.traineddata` fails to open even though it shipped. The directory
    /// handed to Tesseract must never carry the verbatim prefix.
    #[test]
    fn verbatim_tessdata_dir_is_unprefixed_for_tesseract() {
        assert_eq!(
            super::tessdata_dir_arg(std::path::Path::new(
                r"\\?\C:\Users\alice\AppData\Local\TennoScope\tesseract"
            )),
            std::path::PathBuf::from(r"C:\Users\alice\AppData\Local\TennoScope\tesseract"),
        );
        assert_eq!(
            super::tessdata_dir_arg(std::path::Path::new(r"\\?\UNC\server\share\tesseract")),
            std::path::PathBuf::from(r"\\server\share\tesseract"),
        );
        assert_eq!(
            super::tessdata_dir_arg(std::path::Path::new(r"C:\normal\tesseract")),
            std::path::PathBuf::from(r"C:\normal\tesseract"),
        );
        assert_eq!(
            super::tessdata_dir_arg(std::path::Path::new("/usr/share/tessdata")),
            std::path::PathBuf::from("/usr/share/tessdata"),
        );
    }
}
