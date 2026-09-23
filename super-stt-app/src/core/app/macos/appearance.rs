// SPDX-License-Identifier: GPL-3.0-only
//! Light or dark, the accent color, and frosted glass, as macOS sets them.
//!
//! On COSMIC, libcosmic follows the desktop's own theme settings. A Mac has no
//! such settings, so libcosmic would stay dark and teal whatever macOS is set
//! to. On macOS the app follows the system instead: it opens in the
//! appearance and accent color chosen in System Settings, and changes when
//! they do — including when Auto switches the appearance at dusk and dawn.
//!
//! Appearance changes come from the application's `effectiveAppearance`,
//! which tracks the system. The window's does not: iced sets the window's
//! appearance to match the app's theme, so the title bar and traffic lights
//! agree with it, and a window with an appearance of its own no longer
//! follows the system — nor does winit report system changes for it. Accent
//! changes come as `AppKit`'s system-colors notification.
//!
//! Frosted glass is libcosmic's own: the translucent surfaces COSMIC gives a
//! window over a blurred backdrop. libcosmic turns it on only where a Wayland
//! compositor offers the blur, so on macOS the app bakes those surfaces into
//! the theme itself, over a blurred backdrop of macOS's own (see `macos`).
//! Reduce Transparency, in the accessibility settings, turns it off, as it
//! does for Mac apps' own translucency.

use std::cell::{Cell, OnceCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, LazyLock, OnceLock};

use block2::RcBlock;
use cosmic::cosmic_theme::palette::color_difference::Wcag21RelativeContrast;
use cosmic::cosmic_theme::palette::{Clamp, FromColor, IntoColor, Oklch, Srgb, Srgba};
use cosmic::cosmic_theme::{Density, ThemeBuilder};
use cosmic::iced::futures::{SinkExt, Stream};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSColor,
    NSColorSpace, NSSystemColorsDidChangeNotification, NSWorkspace,
    NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSNotification,
    NSNotificationCenter, NSNotificationName, NSObject, NSObjectNSKeyValueObserverRegistration,
    NSObjectProtocol, NSOperationQueue, NSString, NSUserDefaults, ns_string,
};
use tokio::sync::watch;

use crate::ui::messages::{Message, ShellMessage};

/// What the app takes from the system's look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    dark: bool,
    /// The accent color, as macOS draws it in this appearance; `None` if it
    /// could not be read, which leaves libcosmic's own.
    accent: Option<Srgb>,
    /// Frosted glass, which Reduce Transparency turns off.
    frosted: bool,
}

impl Look {
    /// Whether the window is frosted glass: see-through, over a blur.
    pub(crate) fn frosted(self) -> bool {
        self.frosted
    }

    /// This look with frosted glass off.
    pub(crate) fn unfrosted(self) -> Self {
        Self {
            frosted: false,
            ..self
        }
    }

    /// The theme for this look: libcosmic's light or dark, around the
    /// system's accent color, frosted unless the system asks otherwise, at
    /// COSMIC's compact density — nearer the spacing of a Mac app than its
    /// standard one.
    pub(crate) fn theme(self) -> cosmic::Theme {
        let mut builder = if self.dark {
            ThemeBuilder::dark()
        } else {
            ThemeBuilder::light()
        }
        .spacing(Density::Compact.into());
        if let Some(accent) = self.accent {
            builder = builder.accent(accent);
        }
        if self.frosted {
            builder = frost(builder, self.dark);
        }
        let mut theme = builder.build();
        if let Some(accent) = self.accent {
            // libcosmic derives accent text too, but for most macOS accents on
            // its light surfaces finds no shade of them that reads, and uses
            // black.
            let surfaces = [
                theme.bg_color().color,
                theme.primary_container_color().color,
            ];
            theme.accent_text = Some(readable_on(accent, &surfaces, self.dark).into());
        }
        cosmic::Theme::custom(Arc::new(theme))
    }
}

/// libcosmic's frosted-glass surfaces, as its theme builder makes them for a
/// blurred window: the background at the blur strength's opacity, and the
/// containers on it more opaque by the builder's own margins.
fn frost(builder: ThemeBuilder, dark: bool) -> ThemeBuilder {
    let opaque = builder.clone().build();
    let alpha = builder.alpha_map.blurred_alpha(builder.frosted);
    let (primary, secondary) = if dark { (0.3, 0.6) } else { (0.25, 0.5) };
    let with_alpha = |color: Srgba, alpha: f32| Srgba {
        alpha: alpha.min(1.0),
        ..color
    };
    ThemeBuilder {
        bg_color: Some(with_alpha(opaque.bg_color(), alpha)),
        primary_container_bg: Some(with_alpha(
            opaque.primary_container_color(),
            alpha + primary,
        )),
        secondary_container_bg: Some(with_alpha(
            opaque.secondary_container_color(),
            alpha + secondary,
        )),
        ..builder
    }
}

/// Whether the accessibility settings leave room for translucency.
fn translucency_allowed() -> bool {
    !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency()
}

/// Contrast libcosmic asks of accent text against the surfaces it sits on.
const TEXT_CONTRAST: f32 = 4.0;

/// `accent`, darkened on light surfaces or lightened on dark ones just enough
/// to read as text on all of `surfaces`, keeping its hue — as macOS darkens
/// its own blue for links. As close to white or black as it gets, if no shade
/// reads.
fn readable_on(accent: Srgb, surfaces: &[Srgb], dark: bool) -> Srgb {
    let reads = |color: Srgb| {
        surfaces
            .iter()
            .all(|surface| surface.relative_contrast(color) >= TEXT_CONTRAST)
    };
    let step = if dark { 0.01 } else { -0.01 };
    let mut shade: Oklch = accent.into_color();
    let mut color = accent;
    while !reads(color) && (0.0..=1.0).contains(&(shade.l + step)) {
        shade.l += step;
        color = Srgb::from_color(shade).clamp();
    }
    color
}

/// The look the app opened in; see [`launch_look`].
static LAUNCH: OnceLock<Look> = OnceLock::new();

/// The system's look as last observed. [`changes`] watches it.
static SYSTEM: LazyLock<watch::Sender<Option<Look>>> = LazyLock::new(|| watch::Sender::new(None));

/// The system's look when the app opened, read without the application
/// object — which winit has yet to create, and must create itself — so the
/// app opens in it rather than switching after its first frame.
///
/// `AppleInterfaceStyle` is present, as `Dark`, only while the system is dark,
/// Auto included.
pub(crate) fn launch_look() -> Look {
    *LAUNCH.get_or_init(|| {
        let style =
            NSUserDefaults::standardUserDefaults().stringForKey(ns_string!("AppleInterfaceStyle"));
        let dark = style.is_some_and(|style| style.to_string() == "Dark");
        // SAFETY: AppKit's own constants.
        let name = unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        let accent = NSAppearance::appearanceNamed(name).and_then(|appearance| accent(&appearance));
        Look {
            dark,
            accent,
            frosted: translucency_allowed(),
        }
    })
}

/// The system's look now, from the application object, which follows it.
fn system_look(mtm: MainThreadMarker) -> Option<Look> {
    let appearance = NSApplication::sharedApplication(mtm).effectiveAppearance();
    // SAFETY: AppKit's own constants.
    let (light, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let name =
        appearance.bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[light, dark]))?;
    Some(Look {
        dark: *name == *dark,
        accent: accent(&appearance),
        frosted: translucency_allowed(),
    })
}

/// The accent color as drawn in `appearance`: macOS gives each appearance its
/// own shade of it.
fn accent(appearance: &NSAppearance) -> Option<Srgb> {
    let accent = Cell::new(None);
    appearance.performAsCurrentDrawingAppearance(&RcBlock::new(|| {
        let color =
            NSColor::controlAccentColor().colorUsingColorSpace(&NSColorSpace::sRGBColorSpace());
        #[allow(clippy::cast_possible_truncation)]
        accent.set(color.map(|color| {
            Srgb::new(
                color.redComponent() as f32,
                color.greenComponent() as f32,
                color.blueComponent() as f32,
            )
        }));
    }));
    accent.get()
}

/// Record the system's look, waking [`changes`] if it is new.
fn publish(mtm: MainThreadMarker) {
    let look = system_look(mtm);
    SYSTEM.send_if_modified(|last| {
        if *last == look {
            return false;
        }
        log::debug!("system look: {look:?}");
        *last = look;
        true
    });
}

define_class!(
    /// Observes the application's `effectiveAppearance`.
    #[unsafe(super(NSObject))]
    #[name = "SuperSttAppearanceObserver"]
    #[thread_kind = MainThreadOnly]
    struct AppearanceObserver;

    unsafe impl NSObjectProtocol for AppearanceObserver {}

    impl AppearanceObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            publish(self.mtm());
        }
    }
);

thread_local! {
    /// The registered observer. It observes the application, which lives as
    /// long as the process, so it is never unregistered or freed.
    static OBSERVER: OnceCell<Retained<AppearanceObserver>> = const { OnceCell::new() };
}

/// Start following the system's look, once.
///
/// Must run on the main thread, after launch: the application object is
/// winit's by then, and creating it any earlier would preempt winit's.
pub(crate) fn observe() {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("not following the system's look: off the main thread");
        return;
    };
    OBSERVER.with(|registered| {
        if registered.get().is_some() {
            return;
        }
        let observer = mtm.alloc::<AppearanceObserver>().set_ivars(());
        // SAFETY: `init` is `NSObject`'s initializer, which the class keeps.
        let observer: Retained<AppearanceObserver> = unsafe { msg_send![super(observer), init] };
        // SAFETY: the observer implements `observeValueForKeyPath:…`, and is
        // kept for the life of the process, so it outlives the registration.
        // The context is unused, so null.
        unsafe {
            NSApplication::sharedApplication(mtm).addObserver_forKeyPath_options_context(
                &observer,
                ns_string!("effectiveAppearance"),
                // `Initial` reports the current look straight away, which
                // corrects the launch look should it have read differently.
                NSKeyValueObservingOptions::Initial | NSKeyValueObservingOptions::New,
                std::ptr::null_mut(),
            );
        }
        let _ = registered.set(observer);

        // Accent and accessibility changes. On the main queue, where the
        // look is read.
        let changed = RcBlock::new(|_: NonNull<NSNotification>| {
            if let Some(mtm) = MainThreadMarker::new() {
                publish(mtm);
            }
        });
        let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
        let notifications: [(&NSNotificationCenter, &NSNotificationName); 2] = [
            // SAFETY: AppKit's own constants.
            (&NSNotificationCenter::defaultCenter(), unsafe {
                NSSystemColorsDidChangeNotification
            }),
            (&workspace, unsafe {
                NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification
            }),
        ];
        for (center, name) in notifications {
            // SAFETY: the block runs on the main queue, where its AppKit calls
            // must. The center keeps the registration for the life of the
            // process.
            unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    None,
                    Some(&NSOperationQueue::mainQueue()),
                    &changed,
                );
            }
        }
    });
}

/// Each new system look, as a message.
pub(crate) fn changes() -> impl Stream<Item = Message> {
    cosmic::iced::stream::channel(1, async |mut output| {
        let mut system = SYSTEM.subscribe();
        system.mark_changed();
        // The sender is a static, so it is never dropped and this never ends.
        while system.changed().await.is_ok() {
            let Some(look) = *system.borrow_and_update() else {
                continue;
            };
            let message = Message::Shell(ShellMessage::SystemLook(look));
            if output.send(message).await.is_err() {
                break;
            }
        }
    })
}
