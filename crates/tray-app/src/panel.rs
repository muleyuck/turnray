//! The panel the tray icon opens: the list `ui::panel_model` works out, display-only,
//! with the ⚙ and Quit buttons under it. Nothing here decides what to show.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplication, NSApplicationDidResignActiveNotification, NSBackingStoreType, NSBox, NSBoxType,
    NSButton, NSColor, NSEvent, NSEventMask, NSFont, NSImage, NSImageView, NSLayoutAttribute,
    NSLayoutConstraint, NSLayoutConstraintOrientation, NSLayoutPriorityDefaultLow, NSLineBreakMode,
    NSPanel, NSPopUpMenuWindowLevel, NSScreen, NSScrollView, NSStackView, NSStackViewGravity,
    NSStatusBarButton, NSStatusItem, NSTextAlignment, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowCollectionBehavior,
    NSWindowDidMoveNotification, NSWindowDidResizeNotification, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSData, NSEdgeInsets, NSNotification, NSNotificationCenter, NSObject, NSPoint, NSRect,
    NSSize, NSString,
};
use tao::event_loop::EventLoopProxy;

use crate::backend::UserEvent;
use crate::images;
use crate::ui::{self, AgentCard, PanelContent, PanelGroup, Rect};

const WIDTH: f64 = 320.0;
const CORNER_RADIUS: f64 = 10.0;
const INSET: f64 = 12.0;
/// How far a card sits in from its group's header.
const CARD_INDENT: f64 = 26.0;
const ICON_SIZE: f64 = 18.0;
/// The list never takes more of the screen than this, so the buttons stay on it.
const MAX_SCREEN_SHARE: f64 = 0.6;
const ESCAPE_KEY_CODE: u16 = 53;

struct TargetIvars {
    proxy: EventLoopProxy<UserEvent>,
}

define_class!(
    /// Receives the buttons' clicks and passes them to the event loop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TurnrayPanelTarget"]
    #[ivars = TargetIvars]
    struct Target;

    impl Target {
        #[unsafe(method(settings:))]
        fn settings(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().proxy.send_event(UserEvent::Settings);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().proxy.send_event(UserEvent::Quit);
        }
    }

    unsafe impl NSObjectProtocol for Target {}
);

impl Target {
    fn new(proxy: EventLoopProxy<UserEvent>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { proxy });
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    /// The list's document view. Flipped, so the list hangs from the top: it opens at its
    /// first line and keeps its scroll position when its height changes.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TurnrayFlippedView"]
    struct FlippedView;

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

define_class!(
    /// The panel's window: borderless, so it has no title bar and no arrow. A borderless
    /// window refuses to become key, which would keep Esc from reaching it.
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "TurnrayPanelWindow"]
    struct PanelWindow;

    impl PanelWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }
    }
);

impl PanelWindow {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, 100.0));
        let window: Retained<Self> = unsafe {
            msg_send![
                Self::alloc(mtm),
                initWithContentRect: rect,
                styleMask: NSWindowStyleMask::Borderless,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        // It is shown and hidden, never closed, so it must outlive `orderOut`.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setLevel(NSPopUpMenuWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                // Left out of Mission Control and of window cycling, like a menu.
                | NSWindowCollectionBehavior::Transient
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        // Closing is ours (see `Panel::watch`), not AppKit's on deactivation.
        window.setHidesOnDeactivate(false);
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        window
    }
}

/// The panel's background: the translucent material popovers use, with rounded corners.
fn background(mtm: MainThreadMarker) -> Retained<NSVisualEffectView> {
    let effect = NSVisualEffectView::new(mtm);
    effect.setMaterial(NSVisualEffectMaterial::Popover);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    effect.setWantsLayer(true);
    // Through the layer by message: typing it would pull in QuartzCore for two setters.
    unsafe {
        let layer: Option<Retained<AnyObject>> = msg_send![&*effect, layer];
        if let Some(layer) = layer {
            let _: () = msg_send![&*layer, setCornerRadius: CORNER_RADIUS];
            let _: () = msg_send![&*layer, setMasksToBounds: true];
        }
    }
    effect
}

fn rect(r: NSRect) -> Rect {
    Rect {
        x: r.origin.x,
        y: r.origin.y,
        width: r.size.width,
        height: r.size.height,
    }
}

/// What is registered while the panel is open, to close it or move it with the icon.
struct Watchers {
    monitors: Vec<Retained<AnyObject>>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

pub struct Panel {
    mtm: MainThreadMarker,
    proxy: EventLoopProxy<UserEvent>,
    window: Retained<PanelWindow>,
    root: Retained<NSView>,
    list: Retained<NSStackView>,
    list_height: Retained<NSLayoutConstraint>,
    settings_button: Retained<NSButton>,
    /// Kept alive here: a button holds its target weakly.
    _target: Retained<Target>,
    /// The tray button the panel hangs from, while it is open.
    anchor: Option<Retained<NSStatusBarButton>>,
    watchers: Option<Watchers>,
}

fn label(
    text: &str,
    font: &NSFont,
    color: &NSColor,
    mtm: MainThreadMarker,
) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(font));
    l.setTextColor(Some(color));
    l.setUsesSingleLineMode(true);
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    // Below the width the panel gives it, so a long line is cut rather than widening it.
    l.setContentCompressionResistancePriority_forOrientation(
        NSLayoutPriorityDefaultLow,
        NSLayoutConstraintOrientation::Horizontal,
    );
    l
}

/// A template image, so AppKit tints it to the text colour like the menu bar does.
fn icon(png: &[u8], mtm: MainThreadMarker) -> Retained<NSImageView> {
    let image =
        NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(png)).expect("embedded PNG");
    image.setTemplate(true);
    image.setSize(NSSize::new(ICON_SIZE, ICON_SIZE));
    let view = NSImageView::imageViewWithImage(&image, mtm);
    view.setContentTintColor(Some(&NSColor::labelColor()));
    activate(&[
        view.widthAnchor().constraintEqualToConstant(ICON_SIZE),
        view.heightAnchor().constraintEqualToConstant(ICON_SIZE),
    ]);
    view
}

fn stack(
    orientation: NSUserInterfaceLayoutOrientation,
    views: &[&NSView],
    mtm: MainThreadMarker,
) -> Retained<NSStackView> {
    let s = NSStackView::stackViewWithViews(&NSArray::from_slice(views), mtm);
    s.setOrientation(orientation);
    s
}

fn activate(constraints: &[Retained<NSLayoutConstraint>]) {
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(constraints));
}

/// Makes `view` as wide as `list` less its insets, so every row shares one width.
fn fill_width(view: &NSView, list: &NSStackView, indent: f64) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate(&[view
        .widthAnchor()
        .constraintEqualToAnchor_constant(&list.widthAnchor(), -(2.0 * INSET + indent))]);
}

fn group_views(group: &PanelGroup, mtm: MainThreadMarker) -> Vec<Retained<NSView>> {
    let name = label(
        group.status.as_str(),
        &NSFont::boldSystemFontOfSize(13.0),
        &NSColor::labelColor(),
        mtm,
    );
    let count = label(
        &group.count.to_string(),
        &NSFont::systemFontOfSize(13.0),
        &NSColor::secondaryLabelColor(),
        mtm,
    );
    let image = icon(images::status_png(group.status), mtm);
    let header = stack(
        NSUserInterfaceLayoutOrientation::Horizontal,
        &[&image, &name, &count],
        mtm,
    );
    header.setSpacing(6.0);
    let mut views: Vec<Retained<NSView>> = vec![Retained::into_super(header)];
    for agent in &group.agents {
        let card = card_view(agent, mtm);
        views.push(Retained::into_super(card));
    }
    views
}

fn card_view(agent: &AgentCard, mtm: MainThreadMarker) -> Retained<NSStackView> {
    let heading = label(
        &agent.heading,
        &NSFont::boldSystemFontOfSize(13.0),
        &NSColor::labelColor(),
        mtm,
    );
    let mut lines: Vec<Retained<NSView>> =
        vec![Retained::into_super(Retained::into_super(heading))];
    if let Some(title) = &agent.title {
        let title = label(
            title,
            &NSFont::systemFontOfSize(11.0),
            &NSColor::secondaryLabelColor(),
            mtm,
        );
        lines.push(Retained::into_super(Retained::into_super(title)));
    }
    let refs: Vec<&NSView> = lines.iter().map(|v| &**v).collect();
    let card = stack(NSUserInterfaceLayoutOrientation::Vertical, &refs, mtm);
    card.setAlignment(NSLayoutAttribute::Leading);
    card.setSpacing(1.0);
    card.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: CARD_INDENT,
        bottom: 0.0,
        right: 0.0,
    });
    card
}

fn message_view(text: &str, warning: bool, mtm: MainThreadMarker) -> Retained<NSStackView> {
    let line = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    line.setAlignment(NSTextAlignment::Center);
    line.setTextColor(Some(&NSColor::secondaryLabelColor()));
    line.setPreferredMaxLayoutWidth(WIDTH - 2.0 * INSET - ICON_SIZE - 8.0);
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setSpacing(8.0);
    let mut views: Vec<Retained<NSView>> = Vec::new();
    if warning {
        views.push(Retained::into_super(Retained::into_super(icon(
            images::error_png(),
            mtm,
        ))));
    }
    views.push(Retained::into_super(Retained::into_super(line)));
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    row.setViews_inGravity(&NSArray::from_slice(&refs), NSStackViewGravity::Center);
    row.setEdgeInsets(NSEdgeInsets {
        top: 12.0,
        left: 0.0,
        bottom: 12.0,
        right: 0.0,
    });
    row
}

fn separator(mtm: MainThreadMarker) -> Retained<NSBox> {
    let b = NSBox::new(mtm);
    b.setBoxType(NSBoxType::Separator);
    b
}

/// `activate` arrived in macOS 14; the app still runs on 11, which only has the older call.
fn activate_app(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    if app.respondsToSelector(sel!(activate)) {
        app.activate();
    } else {
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

impl Panel {
    pub fn new(proxy: EventLoopProxy<UserEvent>, mtm: MainThreadMarker) -> Panel {
        let target = Target::new(proxy.clone(), mtm);

        let list = NSStackView::new(mtm);
        list.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        list.setAlignment(NSLayoutAttribute::Leading);
        list.setSpacing(6.0);
        list.setEdgeInsets(NSEdgeInsets {
            top: INSET,
            left: INSET,
            bottom: INSET,
            right: INSET,
        });
        list.setTranslatesAutoresizingMaskIntoConstraints(false);

        let document = FlippedView::new(mtm);
        document.setTranslatesAutoresizingMaskIntoConstraints(false);
        document.addSubview(&list);

        let scroll = NSScrollView::new(mtm);
        scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&document));
        let clip = scroll.contentView();

        let gear = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("gearshape"),
            Some(&NSString::from_str("Settings")),
        );
        let settings_button = match gear {
            Some(image) => unsafe {
                NSButton::buttonWithImage_target_action(
                    &image,
                    Some(&target),
                    Some(sel!(settings:)),
                    mtm,
                )
            },
            None => unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str("Settings"),
                    Some(&target),
                    Some(sel!(settings:)),
                    mtm,
                )
            },
        };
        let quit_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Quit"),
                Some(&target),
                Some(sel!(quit:)),
                mtm,
            )
        };
        let footer = NSStackView::new(mtm);
        footer.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        let settings_view: &NSView = &settings_button;
        let quit_view: &NSView = &quit_button;
        footer.setViews_inGravity(
            &NSArray::from_slice(&[settings_view]),
            NSStackViewGravity::Leading,
        );
        footer.setViews_inGravity(
            &NSArray::from_slice(&[quit_view]),
            NSStackViewGravity::Trailing,
        );
        footer.setEdgeInsets(NSEdgeInsets {
            top: 8.0,
            left: INSET,
            bottom: 8.0,
            right: INSET,
        });
        footer.setTranslatesAutoresizingMaskIntoConstraints(false);

        let rule = separator(mtm);
        rule.setTranslatesAutoresizingMaskIntoConstraints(false);

        let root = NSView::new(mtm);
        root.addSubview(&scroll);
        root.addSubview(&rule);
        root.addSubview(&footer);

        let list_height = scroll.heightAnchor().constraintEqualToConstant(0.0);
        activate(&[
            root.widthAnchor().constraintEqualToConstant(WIDTH),
            // The list fills the document view, which is as wide as the clip view and as
            // tall as the list.
            list.topAnchor()
                .constraintEqualToAnchor(&document.topAnchor()),
            list.leadingAnchor()
                .constraintEqualToAnchor(&document.leadingAnchor()),
            list.trailingAnchor()
                .constraintEqualToAnchor(&document.trailingAnchor()),
            list.bottomAnchor()
                .constraintEqualToAnchor(&document.bottomAnchor()),
            document
                .topAnchor()
                .constraintEqualToAnchor(&clip.topAnchor()),
            document
                .leadingAnchor()
                .constraintEqualToAnchor(&clip.leadingAnchor()),
            document
                .trailingAnchor()
                .constraintEqualToAnchor(&clip.trailingAnchor()),
            scroll
                .topAnchor()
                .constraintEqualToAnchor(&root.topAnchor()),
            scroll
                .leadingAnchor()
                .constraintEqualToAnchor(&root.leadingAnchor()),
            scroll
                .trailingAnchor()
                .constraintEqualToAnchor(&root.trailingAnchor()),
            list_height.clone(),
            rule.topAnchor()
                .constraintEqualToAnchor(&scroll.bottomAnchor()),
            rule.leadingAnchor()
                .constraintEqualToAnchor(&root.leadingAnchor()),
            rule.trailingAnchor()
                .constraintEqualToAnchor(&root.trailingAnchor()),
            footer
                .topAnchor()
                .constraintEqualToAnchor(&rule.bottomAnchor()),
            footer
                .leadingAnchor()
                .constraintEqualToAnchor(&root.leadingAnchor()),
            footer
                .trailingAnchor()
                .constraintEqualToAnchor(&root.trailingAnchor()),
            footer
                .bottomAnchor()
                .constraintEqualToAnchor(&root.bottomAnchor()),
        ]);

        let effect = background(mtm);
        root.setTranslatesAutoresizingMaskIntoConstraints(false);
        effect.addSubview(&root);
        // Top and sides only: the window is sized to the root, not the other way round.
        activate(&[
            root.topAnchor()
                .constraintEqualToAnchor(&effect.topAnchor()),
            root.leadingAnchor()
                .constraintEqualToAnchor(&effect.leadingAnchor()),
            root.trailingAnchor()
                .constraintEqualToAnchor(&effect.trailingAnchor()),
        ]);
        let window = PanelWindow::new(mtm);
        window.setContentView(Some(&effect));

        Panel {
            mtm,
            proxy,
            window,
            root,
            list,
            list_height,
            settings_button,
            _target: target,
            anchor: None,
            watchers: None,
        }
    }

    pub fn is_shown(&self) -> bool {
        self.window.isVisible()
    }

    /// Replaces the list. Done while closed too, so the panel opens on the latest.
    pub fn set_content(&mut self, content: &PanelContent) {
        for view in self.list.arrangedSubviews().iter() {
            view.removeFromSuperview();
        }
        let mtm = self.mtm;
        match content {
            PanelContent::Empty => {}
            PanelContent::Message { text, warning } => {
                let row = message_view(text, *warning, mtm);
                self.list.addArrangedSubview(&row);
                fill_width(&row, &self.list, 0.0);
            }
            PanelContent::Groups(groups) => {
                for (i, group) in groups.iter().enumerate() {
                    if i > 0 {
                        let rule = separator(mtm);
                        self.list.addArrangedSubview(&rule);
                        fill_width(&rule, &self.list, 0.0);
                    }
                    for view in group_views(group, mtm) {
                        self.list.addArrangedSubview(&view);
                        fill_width(&view, &self.list, 0.0);
                    }
                }
            }
        }
        if self.is_shown() {
            self.resize();
        }
    }

    /// The icon's frame and its screen's, read afresh: a status item keeps its right edge,
    /// so its left edge moves whenever the Full style's image changes width.
    fn placement(&self) -> Option<(Rect, Rect)> {
        let bar = self.anchor.as_ref()?.window()?;
        // The screen whose menu bar was clicked, which need not be the main one.
        let screen = bar.screen().or_else(|| NSScreen::mainScreen(self.mtm))?;
        Some((rect(bar.frame()), rect(screen.visibleFrame())))
    }

    /// Fits the list to its content, up to the limit, and the window to both, hanging
    /// from the icon.
    fn resize(&self) {
        let Some((icon, visible)) = self.placement() else {
            return;
        };
        let content = self.list.fittingSize().height;
        self.list_height
            .setConstant(content.min(visible.height * MAX_SCREEN_SHARE));
        self.root.layoutSubtreeIfNeeded();
        let f = ui::panel_frame(icon, WIDTH, self.root.fittingSize().height, visible);
        self.window.setFrame_display(
            NSRect::new(NSPoint::new(f.x, f.y), NSSize::new(f.width, f.height)),
            true,
        );
        // The shadow follows the rounded corners only once it is redrawn for the new size.
        self.window.invalidateShadow();
    }

    /// Opens the panel under `item`'s button. Does nothing while the item is hidden.
    pub fn show(&mut self, item: &NSStatusItem) {
        let mtm = self.mtm;
        let Some(button) = item.button(mtm) else {
            return;
        };
        let Some(bar) = button.window() else {
            return;
        };
        self.anchor = Some(button);
        self.resize();
        let app = NSApplication::sharedApplication(mtm);
        // The last close may have hidden the app to hand the keyboard back.
        app.unhide(None);
        activate_app(mtm);
        self.window.makeKeyAndOrderFront(None);
        self.keep_highlight();
        self.watch(&bar);
    }

    /// The icon moved or changed width while open: hang from it again.
    pub fn follow_anchor(&self) {
        if self.is_shown() {
            self.resize();
        }
    }

    /// tray-icon unhighlights the button on every mouse up; while open it stays pressed.
    pub fn keep_highlight(&self) {
        if let Some(button) = &self.anchor {
            button.highlight(true);
        }
    }

    /// `hand_back` is for a close the user made inside the app (Esc, the tray icon): the
    /// keyboard goes back to the app that had it. Closed by a click elsewhere or by losing
    /// focus, another app already has it.
    pub fn close(&mut self, hand_back: bool) {
        // Undone even if the window is already gone, so nothing outlives the panel.
        self.unwatch();
        if let Some(button) = self.anchor.take() {
            button.highlight(false);
        }
        if !self.is_shown() {
            return;
        }
        self.window.orderOut(None);
        let app = NSApplication::sharedApplication(self.mtm);
        if hand_back && app.isActive() {
            app.hide(None);
        }
    }

    /// Pops the ⚙ menu up under its button. Returns once the menu closes.
    pub fn show_settings_menu(&self, menu: &tray_icon::menu::Menu) {
        use tray_icon::menu::{dpi::LogicalPosition, ContextMenu};
        let view: &NSView = &self.settings_button;
        // muda assumes an unflipped view; NSButton is flipped, so 0 lands on its bottom edge.
        unsafe {
            menu.show_context_menu_for_nsview(
                (view as *const NSView).cast(),
                Some(LogicalPosition::new(0.0, 0.0).into()),
            );
        }
    }

    fn watch(&mut self, bar: &NSWindow) {
        self.unwatch();
        let send_close = {
            let proxy = self.proxy.clone();
            move |hand_back: bool| {
                let _ = proxy.send_event(UserEvent::ClosePanel { hand_back });
            }
        };
        let mut monitors = Vec::new();
        let on_click = send_close.clone();
        if let Some(m) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
            &RcBlock::new(move |_: NonNull<NSEvent>| on_click(false)),
        ) {
            monitors.push(m);
        }
        let on_key = send_close.clone();
        // Returning null swallows the Esc; anything else goes on to the panel.
        let on_key_down = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            if unsafe { event.as_ref() }.keyCode() == ESCAPE_KEY_CODE {
                on_key(true);
                std::ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });
        let local = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown,
                &on_key_down,
            )
        };
        if let Some(m) = local {
            monitors.push(m);
        }
        let center = NSNotificationCenter::defaultCenter();
        let mut observers = vec![unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidResignActiveNotification),
                None,
                None,
                &RcBlock::new(move |_: NonNull<NSNotification>| send_close(false)),
            )
        }];
        // Sent on, not handled here: the frame is read once the change has settled.
        for name in unsafe { [NSWindowDidResizeNotification, NSWindowDidMoveNotification] } {
            let proxy = self.proxy.clone();
            observers.push(unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    Some(bar),
                    None,
                    &RcBlock::new(move |_: NonNull<NSNotification>| {
                        let _ = proxy.send_event(UserEvent::AnchorMoved);
                    }),
                )
            });
        }
        self.watchers = Some(Watchers {
            monitors,
            observers,
        });
    }

    fn unwatch(&mut self) {
        let Some(w) = self.watchers.take() else {
            return;
        };
        for m in &w.monitors {
            unsafe { NSEvent::removeMonitor(m) };
        }
        let center = NSNotificationCenter::defaultCenter();
        for o in &w.observers {
            let observer: &AnyObject = o.as_ref();
            unsafe { center.removeObserver(observer) };
        }
    }
}
