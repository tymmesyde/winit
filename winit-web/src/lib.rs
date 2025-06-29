//! # Web
//!
//! Winit supports running in Browsers by compiling to WebAssembly with
//! [`wasm-bindgen`][wasm_bindgen]. For information on using Rust on WebAssembly, check out the
//! [Rust and WebAssembly book].
//!
//! The officially supported browsers are Chrome, Firefox and Safari 13.1+, though forks of these
//! should work fine.
//!
//! On the Web platform, a Winit [`Window`] is backed by a [`HTMLCanvasElement`][canvas]. Winit will
//! create that canvas for you or you can [provide your own][with_canvas]. Then you can either let
//! Winit [insert it into the DOM for you][insert], or [retrieve the canvas][get] and insert it
//! yourself.
//!
//! [canvas]: https://developer.mozilla.org/en-US/docs/Web/API/HTMLCanvasElement
//! [with_canvas]: WindowAttributesWeb::with_canvas
//! [get]: WindowExtWeb::canvas
//! [insert]: WindowAttributesWeb::with_append
//! [wasm_bindgen]: https://docs.rs/wasm-bindgen
//! [Rust and WebAssembly book]: https://rustwasm.github.io/book
//!
//! ## CSS properties
//!
//! It is recommended **not** to apply certain CSS properties to the canvas:
//! - [`transform`](https://developer.mozilla.org/en-US/docs/Web/CSS/transform)
//! - [`border`](https://developer.mozilla.org/en-US/docs/Web/CSS/border)
//! - [`padding`](https://developer.mozilla.org/en-US/docs/Web/CSS/padding)
//!
//! The following APIs can't take them into account and will therefore provide inaccurate results:
//! - [`WindowEvent::SurfaceResized`] and [`Window::(set_)surface_size()`]
//! - [`WindowEvent::Occluded`]
//! - [`WindowEvent::PointerMoved`], [`WindowEvent::PointerEntered`] and
//!   [`WindowEvent::PointerLeft`].
//! - [`Window::set_outer_position()`]
//!
//! [`WindowEvent::SurfaceResized`]: crate::event::WindowEvent::SurfaceResized
//! [`Window::(set_)surface_size()`]: crate::window::Window::surface_size
//! [`WindowEvent::Occluded`]: crate::event::WindowEvent::Occluded
//! [`WindowEvent::PointerMoved`]: crate::event::WindowEvent::PointerMoved
//! [`WindowEvent::PointerEntered`]: crate::event::WindowEvent::PointerEntered
//! [`WindowEvent::PointerLeft`]: crate::event::WindowEvent::PointerLeft
//! [`Window::set_outer_position()`]: crate::window::Window::set_outer_position

// Brief introduction to the internals of the Web backend:
// The Web backend used to support both wasm-bindgen and stdweb as methods of binding to the
// environment. Because they are both supporting the same underlying APIs, the actual Web bindings
// are cordoned off into backend abstractions, which present the thinnest unifying layer possible.
//
// When adding support for new events or interactions with the browser, first consult trusted
// documentation (such as MDN) to ensure it is well-standardised and supported across many browsers.
// Once you have decided on the relevant Web APIs, add support to both backends.
//
// The backend is used by the rest of the module to implement Winit's business logic, which forms
// the rest of the code. 'device', 'error', 'monitor', and 'window' define Web-specific structures
// for winit's cross-platform structures. They are all relatively simple translations.
//
// The event_loop module handles listening for and processing events. 'Proxy' implements
// EventLoopProxy and 'WindowTarget' implements ActiveEventLoop. WindowTarget also handles
// registering the event handlers. The 'Execution' struct in the 'runner' module handles taking
// incoming events (from the registered handlers) and ensuring they are passed to the user in a
// compliant way.

macro_rules! os_error {
    ($error:expr) => {{
        winit_core::error::OsError::new(line!(), file!(), $error)
    }};
}

mod r#async;
mod cursor;
mod event;
pub(crate) mod event_loop;
mod lock;
pub(crate) mod main_thread;
mod monitor;
pub(crate) mod web_sys;
pub(crate) mod window;

use std::cell::Ref;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use ::web_sys::HtmlCanvasElement;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use winit_core::application::ApplicationHandler;
use winit_core::cursor::{CustomCursor, CustomCursorSource};
use winit_core::error::NotSupportedError;
use winit_core::event_loop::ActiveEventLoop;
use winit_core::monitor::MonitorHandleProvider;
use winit_core::window::{PlatformWindowAttributes, Window};

pub use self::event_loop::{EventLoop, PlatformSpecificEventLoopAttributes};
use self::web_sys as backend;
use self::window::Window as WebWindow;
use crate::cursor::CustomCursorFuture as PlatformCustomCursorFuture;
use crate::event_loop::ActiveEventLoop as WebActiveEventLoop;
use crate::main_thread::{MainThreadMarker, MainThreadSafe};
use crate::monitor::{
    HasMonitorPermissionFuture as PlatformHasMonitorPermissionFuture,
    MonitorHandle as WebMonitorHandle, MonitorPermissionFuture as PlatformMonitorPermissionFuture,
    OrientationLockFuture as PlatformOrientationLockFuture,
};

pub trait WindowExtWeb {
    /// Only returns the canvas if called from inside the window context (the
    /// main thread).
    fn canvas(&self) -> Option<Ref<'_, HtmlCanvasElement>>;

    /// Returns [`true`] if calling `event.preventDefault()` is enabled.
    ///
    /// See [`WindowExtWeb::set_prevent_default()`] for more details.
    fn prevent_default(&self) -> bool;

    /// Sets whether `event.preventDefault()` should be called on events on the
    /// canvas that have side effects.
    ///
    /// For example, by default using the mouse wheel would cause the page to scroll, enabling this
    /// would prevent that.
    ///
    /// Some events are impossible to prevent. E.g. Firefox allows to access the native browser
    /// context menu with Shift+Rightclick.
    fn set_prevent_default(&self, prevent_default: bool);

    /// Returns whether using [`CursorGrabMode::Locked`] returns raw, un-accelerated mouse input.
    ///
    /// This is the same as [`ActiveEventLoopExtWeb::is_cursor_lock_raw()`], and is provided for
    /// convenience.
    ///
    /// [`CursorGrabMode::Locked`]: crate::window::CursorGrabMode::Locked
    fn is_cursor_lock_raw(&self) -> bool;
}

impl WindowExtWeb for dyn Window + '_ {
    #[inline]
    fn canvas(&self) -> Option<Ref<'_, HtmlCanvasElement>> {
        self.cast_ref::<WebWindow>().expect("non Web window on Web").canvas()
    }

    fn prevent_default(&self) -> bool {
        self.cast_ref::<WebWindow>().expect("non Web window on Web").prevent_default()
    }

    fn set_prevent_default(&self, prevent_default: bool) {
        self.cast_ref::<WebWindow>()
            .expect("non Web window on Web")
            .set_prevent_default(prevent_default)
    }

    fn is_cursor_lock_raw(&self) -> bool {
        self.cast_ref::<WebWindow>().expect("non Web window on Web").is_cursor_lock_raw()
    }
}

#[derive(Clone, Debug)]
pub struct WindowAttributesWeb {
    pub(crate) canvas: Option<Arc<MainThreadSafe<backend::RawCanvasType>>>,
    pub(crate) prevent_default: bool,
    pub(crate) focusable: bool,
    pub(crate) append: bool,
}

impl WindowAttributesWeb {
    /// Pass an [`HtmlCanvasElement`] to be used for this [`Window`]. If [`None`],
    /// the default one will be created.
    ///
    /// In any case, the canvas won't be automatically inserted into the Web page.
    ///
    /// [`None`] by default.
    pub fn with_canvas(mut self, canvas: Option<HtmlCanvasElement>) -> Self {
        match canvas {
            Some(canvas) => {
                let main_thread = MainThreadMarker::new()
                    .expect("received a `HtmlCanvasElement` outside the window context");
                self.canvas = Some(Arc::new(MainThreadSafe::new(main_thread, canvas)));
            },
            None => self.canvas = None,
        }

        self
    }

    /// Sets whether `event.preventDefault()` should be called on events on the
    /// canvas that have side effects.
    ///
    /// See [`WindowExtWeb::set_prevent_default()`] for more details.
    ///
    /// Enabled by default.
    pub fn with_prevent_default(mut self, prevent_default: bool) -> Self {
        self.prevent_default = prevent_default;
        self
    }

    /// Whether the canvas should be focusable using the tab key. This is necessary to capture
    /// canvas keyboard events.
    ///
    /// Enabled by default.
    pub fn with_focusable(mut self, focusable: bool) -> Self {
        self.focusable = focusable;
        self
    }

    /// On window creation, append the canvas element to the Web page if it isn't already.
    ///
    /// Disabled by default.
    pub fn with_append(mut self, append: bool) -> Self {
        self.append = append;
        self
    }
}

impl PlatformWindowAttributes for WindowAttributesWeb {
    fn box_clone(&self) -> Box<dyn PlatformWindowAttributes> {
        Box::from(self.clone())
    }
}

impl PartialEq for WindowAttributesWeb {
    fn eq(&self, other: &Self) -> bool {
        (match (&self.canvas, &other.canvas) {
            (Some(this), Some(other)) => Arc::ptr_eq(this, other),
            (None, None) => true,
            _ => false,
        }) && self.prevent_default.eq(&other.prevent_default)
            && self.focusable.eq(&other.focusable)
            && self.append.eq(&other.append)
    }
}

impl Default for WindowAttributesWeb {
    fn default() -> Self {
        Self { canvas: None, prevent_default: true, focusable: true, append: false }
    }
}

/// Additional methods on `EventLoop` that are specific to the Web.
pub trait EventLoopExtWeb {
    /// Initializes the winit event loop.
    ///
    /// Unlike
    #[cfg_attr(target_feature = "exception-handling", doc = "`run_app()`")]
    #[cfg_attr(
        not(target_feature = "exception-handling"),
        doc = "[`run_app()`]"
    )]
    /// [^1], this returns immediately, and doesn't throw an exception in order to
    /// satisfy its [`!`] return type.
    ///
    /// Once the event loop has been destroyed, it's possible to reinitialize another event loop
    /// by calling this function again. This can be useful if you want to recreate the event loop
    /// while the WebAssembly module is still loaded. For example, this can be used to recreate the
    /// event loop when switching between tabs on a single page application.
    #[rustfmt::skip]
    ///
    #[cfg_attr(
        not(target_feature = "exception-handling"),
        doc = "[`run_app()`]: EventLoop::run_app()"
    )]
    /// [^1]: `run_app()` is _not_ available on Wasm when the target supports `exception-handling`.
    fn spawn_app<A: ApplicationHandler + 'static>(self, app: A);

    /// Sets the strategy for [`ControlFlow::Poll`].
    ///
    /// See [`PollStrategy`].
    ///
    /// [`ControlFlow::Poll`]: crate::event_loop::ControlFlow::Poll
    fn set_poll_strategy(&self, strategy: PollStrategy);

    /// Gets the strategy for [`ControlFlow::Poll`].
    ///
    /// See [`PollStrategy`].
    ///
    /// [`ControlFlow::Poll`]: crate::event_loop::ControlFlow::Poll
    fn poll_strategy(&self) -> PollStrategy;

    /// Sets the strategy for [`ControlFlow::WaitUntil`].
    ///
    /// See [`WaitUntilStrategy`].
    ///
    /// [`ControlFlow::WaitUntil`]: crate::event_loop::ControlFlow::WaitUntil
    fn set_wait_until_strategy(&self, strategy: WaitUntilStrategy);

    /// Gets the strategy for [`ControlFlow::WaitUntil`].
    ///
    /// See [`WaitUntilStrategy`].
    ///
    /// [`ControlFlow::WaitUntil`]: crate::event_loop::ControlFlow::WaitUntil
    fn wait_until_strategy(&self) -> WaitUntilStrategy;

    /// Returns if the users device has multiple screens. Useful to check before prompting the user
    /// with [`EventLoopExtWeb::request_detailed_monitor_permission()`].
    ///
    /// Browsers might always return [`false`] to reduce fingerprinting.
    fn has_multiple_screens(&self) -> Result<bool, NotSupportedError>;

    /// Prompts the user for permission to query detailed information about available monitors. The
    /// returned [`MonitorPermissionFuture`] can be dropped without aborting the request.
    ///
    /// Check [`EventLoopExtWeb::has_multiple_screens()`] before unnecessarily prompting the user
    /// for such permissions.
    ///
    /// [`MonitorHandle`]s don't automatically make use of this after permission is granted. New
    /// [`MonitorHandle`]s have to be created instead.
    ///
    /// [`MonitorHandle`]: crate::monitor::MonitorHandle
    fn request_detailed_monitor_permission(&self) -> MonitorPermissionFuture;

    /// Returns whether the user has given permission to access detailed monitor information.
    ///
    /// [`MonitorHandle`]s don't automatically make use of detailed monitor information after
    /// permission is granted. New [`MonitorHandle`]s have to be created instead.
    ///
    /// [`MonitorHandle`]: crate::monitor::MonitorHandle
    ///
    /// [`MonitorHandle`]: crate::monitor::MonitorHandle
    fn has_detailed_monitor_permission(&self) -> HasMonitorPermissionFuture;
}

pub trait ActiveEventLoopExtWeb {
    /// Sets the strategy for [`ControlFlow::Poll`].
    ///
    /// See [`PollStrategy`].
    ///
    /// [`ControlFlow::Poll`]: crate::event_loop::ControlFlow::Poll
    fn set_poll_strategy(&self, strategy: PollStrategy);

    /// Gets the strategy for [`ControlFlow::Poll`].
    ///
    /// See [`PollStrategy`].
    ///
    /// [`ControlFlow::Poll`]: crate::event_loop::ControlFlow::Poll
    fn poll_strategy(&self) -> PollStrategy;

    /// Sets the strategy for [`ControlFlow::WaitUntil`].
    ///
    /// See [`WaitUntilStrategy`].
    ///
    /// [`ControlFlow::WaitUntil`]: crate::event_loop::ControlFlow::WaitUntil
    fn set_wait_until_strategy(&self, strategy: WaitUntilStrategy);

    /// Gets the strategy for [`ControlFlow::WaitUntil`].
    ///
    /// See [`WaitUntilStrategy`].
    ///
    /// [`ControlFlow::WaitUntil`]: crate::event_loop::ControlFlow::WaitUntil
    fn wait_until_strategy(&self) -> WaitUntilStrategy;

    /// Async version of [`ActiveEventLoop::create_custom_cursor()`] which waits until the
    /// cursor has completely finished loading.
    fn create_custom_cursor_async(&self, source: CustomCursorSource) -> CustomCursorFuture;

    /// Returns whether using [`CursorGrabMode::Locked`] returns raw, un-accelerated mouse input.
    ///
    /// [`CursorGrabMode::Locked`]: crate::window::CursorGrabMode::Locked
    fn is_cursor_lock_raw(&self) -> bool;

    /// Returns if the users device has multiple screens. Useful to check before prompting the user
    /// with [`EventLoopExtWeb::request_detailed_monitor_permission()`].
    ///
    /// Browsers might always return [`false`] to reduce fingerprinting.
    fn has_multiple_screens(&self) -> Result<bool, NotSupportedError>;

    /// Prompts the user for permission to query detailed information about available monitors. The
    /// returned [`MonitorPermissionFuture`] can be dropped without aborting the request.
    ///
    /// Check [`EventLoopExtWeb::has_multiple_screens()`] before unnecessarily prompting the user
    /// for such permissions.
    ///
    /// [`MonitorHandle`]s don't automatically make use of this after permission is granted. New
    /// [`MonitorHandle`]s have to be created instead.
    ///
    /// [`MonitorHandle`]: crate::monitor::MonitorHandle
    fn request_detailed_monitor_permission(&self) -> MonitorPermissionFuture;

    /// Returns whether the user has given permission to access detailed monitor information.
    ///
    /// [`MonitorHandle`]s don't automatically make use of detailed monitor information after
    /// permission is granted. New [`MonitorHandle`]s have to be created instead.
    ///
    /// [`MonitorHandle`]: crate::monitor::MonitorHandle
    fn has_detailed_monitor_permission(&self) -> bool;
}

impl ActiveEventLoopExtWeb for dyn ActiveEventLoop + '_ {
    #[inline]
    fn create_custom_cursor_async(&self, source: CustomCursorSource) -> CustomCursorFuture {
        let event_loop = self.cast_ref::<WebActiveEventLoop>().expect("non Web event loop on Web");
        event_loop.create_custom_cursor_async(source)
    }

    #[inline]
    fn set_poll_strategy(&self, strategy: PollStrategy) {
        let event_loop = self.cast_ref::<WebActiveEventLoop>().expect("non Web event loop on Web");
        event_loop.set_poll_strategy(strategy);
    }

    #[inline]
    fn poll_strategy(&self) -> PollStrategy {
        let event_loop = self.cast_ref::<WebActiveEventLoop>().expect("non Web event loop on Web");
        event_loop.poll_strategy()
    }

    #[inline]
    fn set_wait_until_strategy(&self, strategy: WaitUntilStrategy) {
        let event_loop = self.cast_ref::<WebActiveEventLoop>().expect("non Web event loop on Web");
        event_loop.set_wait_until_strategy(strategy);
    }

    #[inline]
    fn wait_until_strategy(&self) -> WaitUntilStrategy {
        let event_loop = self.cast_ref::<WebActiveEventLoop>().expect("non Web event loop on Web");
        event_loop.wait_until_strategy()
    }

    #[inline]
    fn is_cursor_lock_raw(&self) -> bool {
        let event_loop = self.cast_ref::<WebActiveEventLoop>().expect("non Web event loop on Web");
        event_loop.is_cursor_lock_raw()
    }

    #[inline]
    fn has_multiple_screens(&self) -> Result<bool, NotSupportedError> {
        let event_loop = self.cast_ref::<WebActiveEventLoop>().expect("non Web event loop on Web");
        event_loop.has_multiple_screens()
    }

    #[inline]
    fn request_detailed_monitor_permission(&self) -> MonitorPermissionFuture {
        let event_loop = self.cast_ref::<WebActiveEventLoop>().expect("non Web event loop on Web");
        MonitorPermissionFuture(event_loop.request_detailed_monitor_permission())
    }

    #[inline]
    fn has_detailed_monitor_permission(&self) -> bool {
        let event_loop = self.cast_ref::<WebActiveEventLoop>().expect("non Web event loop on Web");
        event_loop.has_detailed_monitor_permission()
    }
}

/// Strategy used for [`ControlFlow::Poll`][crate::event_loop::ControlFlow::Poll].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum PollStrategy {
    /// Uses [`Window.requestIdleCallback()`] to queue the next event loop. If not available
    /// this will fallback to [`setTimeout()`].
    ///
    /// This strategy will wait for the browser to enter an idle period before running and might
    /// be affected by browser throttling.
    ///
    /// [`Window.requestIdleCallback()`]: https://developer.mozilla.org/en-US/docs/Web/API/Window/requestIdleCallback
    /// [`setTimeout()`]: https://developer.mozilla.org/en-US/docs/Web/API/setTimeout
    IdleCallback,
    /// Uses the [Prioritized Task Scheduling API] to queue the next event loop. If not available
    /// this will fallback to [`setTimeout()`].
    ///
    /// This strategy will run as fast as possible without disturbing users from interacting with
    /// the page and is not affected by browser throttling.
    ///
    /// This is the default strategy.
    ///
    /// [Prioritized Task Scheduling API]: https://developer.mozilla.org/en-US/docs/Web/API/Prioritized_Task_Scheduling_API
    /// [`setTimeout()`]: https://developer.mozilla.org/en-US/docs/Web/API/setTimeout
    #[default]
    Scheduler,
}

/// Strategy used for [`ControlFlow::WaitUntil`][crate::event_loop::ControlFlow::WaitUntil].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum WaitUntilStrategy {
    /// Uses the [Prioritized Task Scheduling API] to queue the next event loop. If not available
    /// this will fallback to [`setTimeout()`].
    ///
    /// This strategy is commonly not affected by browser throttling unless the window is not
    /// focused.
    ///
    /// This is the default strategy.
    ///
    /// [Prioritized Task Scheduling API]: https://developer.mozilla.org/en-US/docs/Web/API/Prioritized_Task_Scheduling_API
    /// [`setTimeout()`]: https://developer.mozilla.org/en-US/docs/Web/API/setTimeout
    #[default]
    Scheduler,
    /// Equal to [`Scheduler`][Self::Scheduler] but wakes up the event loop from a [worker].
    ///
    /// This strategy is commonly not affected by browser throttling regardless of window focus.
    ///
    /// [worker]: https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API
    Worker,
}

#[derive(Debug)]
pub struct CustomCursorFuture(pub(crate) PlatformCustomCursorFuture);

impl Future for CustomCursorFuture {
    type Output = Result<CustomCursor, CustomCursorError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map_ok(|cursor| CustomCursor(Arc::new(cursor)))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum CustomCursorError {
    Blob,
    Decode(String),
}

impl Display for CustomCursorError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob => write!(f, "failed to create `Blob`"),
            Self::Decode(error) => write!(f, "failed to decode image: {error}"),
        }
    }
}

impl Error for CustomCursorError {}

/// Can be dropped without aborting the request for detailed monitor permissions.
#[derive(Debug)]
pub struct MonitorPermissionFuture(pub(crate) PlatformMonitorPermissionFuture);

impl Future for MonitorPermissionFuture {
    type Output = Result<(), MonitorPermissionError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MonitorPermissionError {
    /// User has explicitly denied permission to query detailed monitor information.
    Denied,
    /// User has not decided to give permission to query detailed monitor information.
    Prompt,
    /// Browser does not support detailed monitor information.
    Unsupported,
}

impl Display for MonitorPermissionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            MonitorPermissionError::Denied => write!(
                f,
                "User has explicitly denied permission to query detailed monitor information"
            ),
            MonitorPermissionError::Prompt => write!(
                f,
                "User has not decided to give permission to query detailed monitor information"
            ),
            MonitorPermissionError::Unsupported => {
                write!(f, "Browser does not support detailed monitor information")
            },
        }
    }
}

impl Error for MonitorPermissionError {}

#[derive(Debug)]
pub struct HasMonitorPermissionFuture(PlatformHasMonitorPermissionFuture);

impl Future for HasMonitorPermissionFuture {
    type Output = bool;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

/// Additional methods on [`MonitorHandle`] that are specific to the Web.
///
/// [`MonitorHandle`]: crate::monitor::MonitorHandle
pub trait MonitorHandleExtWeb {
    /// Returns whether the screen is internal to the device or external.
    ///
    /// External devices are generally manufactured separately from the device they are attached to
    /// and can be connected and disconnected as needed, whereas internal screens are part of
    /// the device and not intended to be disconnected.
    fn is_internal(&self) -> Option<bool>;

    /// Returns screen orientation data for this monitor.
    fn orientation(&self) -> OrientationData;

    /// Lock the screen orientation. The returned [`OrientationLockFuture`] can be dropped without
    /// aborting the request.
    ///
    /// Will fail if another locking call is in progress.
    fn request_lock(&self, orientation: OrientationLock) -> OrientationLockFuture;

    /// Unlock the screen orientation.
    ///
    /// Will fail if a locking call is in progress.
    fn unlock(&self) -> Result<(), OrientationLockError>;

    /// Returns whether this [`MonitorHandle`] was created using detailed monitor permissions. If
    /// [`false`] will always represent the current monitor the browser window is in instead of a
    /// specific monitor.
    ///
    /// See [`ActiveEventLoopExtWeb::request_detailed_monitor_permission()`].
    ///
    /// [`MonitorHandle`]: crate::monitor::MonitorHandle
    fn is_detailed(&self) -> bool;
}

impl MonitorHandleExtWeb for dyn MonitorHandleProvider + '_ {
    fn is_internal(&self) -> Option<bool> {
        self.cast_ref::<WebMonitorHandle>().unwrap().is_internal()
    }

    fn orientation(&self) -> OrientationData {
        self.cast_ref::<WebMonitorHandle>().unwrap().orientation()
    }

    fn request_lock(&self, orientation_lock: OrientationLock) -> OrientationLockFuture {
        let future = self.cast_ref::<WebMonitorHandle>().unwrap().request_lock(orientation_lock);
        OrientationLockFuture(future)
    }

    fn unlock(&self) -> Result<(), OrientationLockError> {
        self.cast_ref::<WebMonitorHandle>().unwrap().unlock()
    }

    fn is_detailed(&self) -> bool {
        self.cast_ref::<WebMonitorHandle>().unwrap().is_detailed()
    }
}

/// Screen orientation data.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OrientationData {
    /// The orientation.
    pub orientation: Orientation,
    /// [`true`] if the [`orientation`](Self::orientation) is flipped upside down.
    pub flipped: bool,
    /// The most natural orientation for the screen. Computer monitors are commonly naturally
    /// landscape mode, while mobile phones are commonly naturally portrait mode.
    pub natural: Orientation,
}

/// Screen orientation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Orientation {
    /// The screen's aspect ratio has a width greater than the height.
    Landscape,
    /// The screen's aspect ratio has a height greater than the width.
    Portrait,
}

/// Screen orientation lock options. Represents which orientations a user can use.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OrientationLock {
    /// User is free to use any orientation.
    Any,
    /// User is locked to the most upright natural orientation for the screen. Computer monitors
    /// are commonly naturally landscape mode, while mobile phones are commonly
    /// naturally portrait mode.
    Natural,
    /// User is locked to landscape mode.
    Landscape {
        /// - [`None`]: User is locked to both upright or upside down landscape mode.
        /// - [`true`]: User is locked to upright landscape mode.
        /// - [`false`]: User is locked to upside down landscape mode.
        flipped: Option<bool>,
    },
    /// User is locked to portrait mode.
    Portrait {
        /// - [`None`]: User is locked to both upright or upside down portrait mode.
        /// - [`true`]: User is locked to upright portrait mode.
        /// - [`false`]: User is locked to upside down portrait mode.
        flipped: Option<bool>,
    },
}

/// Can be dropped without aborting the request to lock the screen.
#[derive(Debug)]
pub struct OrientationLockFuture(PlatformOrientationLockFuture);

impl Future for OrientationLockFuture {
    type Output = Result<(), OrientationLockError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OrientationLockError {
    Unsupported,
    Busy,
}

impl Display for OrientationLockError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "Locking the screen orientation is not supported"),
            Self::Busy => write!(f, "Another locking call is in progress"),
        }
    }
}

impl Error for OrientationLockError {}
