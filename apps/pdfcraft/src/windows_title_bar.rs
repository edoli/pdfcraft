//! Windows App SDK caption buttons over the existing winit window.
use std::{ffi::c_void, ptr, sync::OnceLock};

use egui::{Context, Rect, ViewportId};
use pdfcraft_ui_egui::theme::Tokens;
use pdfcraft_ui_egui::window_controls::{NativeTitleBar, NativeTitleBarLayout};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_core::{
    AgileReference, GUID, HRESULT, IInspectable, IInspectable_Vtbl, IUnknown, IUnknownImpl, Interface, Result, RuntimeName, factory, implement, s, w,
};
use windows_sys::Win32::System::{
    LibraryLoader::{GetModuleHandleW, GetProcAddress},
    Memory::{GetProcessHeap, HeapFree},
};

pub(super) fn attach(cc: &eframe::CreationContext<'_>, enabled: bool) -> Result<()> {
    let RawWindowHandle::Win32(handle) = cc.window_handle().map_err(|_| HRESULT(0x80004005_u32 as i32))?.as_raw() else {
        return Err(HRESULT(0x80004005_u32 as i32).into());
    };
    initialize_runtime()?;
    let id = handle.hwnd.get() as u64;
    let bar = window_title_bar(id)?;
    let bar_ref = AgileReference::new(&bar)?;
    set_extended(&bar, enabled)?;
    caption_metrics(&bar, enabled, cc.egui_ctx.pixels_per_point()).publish(&cc.egui_ctx);
    cc.egui_ctx.add_plugin(TitleBar { bar: bar_ref, extended: enabled, failed: false, regions: None, colors: None });
    Ok(())
}

struct TitleBar {
    bar: AgileReference<AppWindowTitleBar>,
    extended: bool,
    failed: bool,
    regions: Option<Vec<PhysicalRect>>,
    colors: Option<[egui::Color32; 5]>,
}

impl egui::Plugin for TitleBar {
    fn debug_name(&self) -> &'static str {
        "Windows App SDK title bar"
    }

    fn input_hook(&mut self, ctx: &Context, input: &mut egui::RawInput) {
        if self.failed || input.viewport_id != ViewportId::ROOT {
            return;
        }
        let system = NativeTitleBarLayout::get(ctx).map_or(!self.extended, |layout| layout.system_title_bar);
        let enabled = !system && !input.viewport().fullscreen.unwrap_or(false);
        let scale = input.viewport().native_pixels_per_point.unwrap_or(1.0) * ctx.zoom_factor();
        let result = self.bar.resolve().and_then(|bar| {
            if enabled != self.extended {
                set_extended(&bar, enabled)?;
                self.extended = enabled;
                self.regions = None;
            }
            caption_metrics(&bar, enabled, scale).publish(ctx);
            Ok(())
        });
        if let Err(error) = result {
            self.restore_system_title_bar(ctx, &error);
        }
    }

    fn output_hook(&mut self, ctx: &Context, _output: &mut egui::FullOutput) {
        if self.failed || ctx.viewport_id() != ViewportId::ROOT {
            return;
        }
        let Some(layout) = NativeTitleBarLayout::get(ctx) else { return };
        if layout.system_title_bar == self.extended && !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
            ctx.request_repaint();
        }
        if !self.extended {
            self.colors = None;
            return;
        }
        let theme = Tokens::get(ctx);
        self.update_colors([theme.titlebar, theme.text, theme.text_muted, theme.hover, theme.pressed]);
        let Some(bar) = NativeTitleBar::get(ctx) else { return };
        let bounds = ctx.content_rect();
        let mut strip = Rect::from_min_max(bounds.min, egui::pos2(bounds.right(), bounds.top() + bar.height));
        strip.min.x += bar.left;
        strip.max.x -= bar.right;
        let mut regions = layout.drag.map(|drag| drag.intersect(strip)).filter(Rect::is_positive).into_iter().collect::<Vec<_>>();
        if let Some(layer) = layout.layer {
            ctx.memory(|memory| {
                for overlay in memory
                    .areas()
                    .visible_layer_ids()
                    .into_iter()
                    .filter(|above| above.order > layer.order && above.order != egui::Order::Tooltip)
                    .filter_map(|above| memory.area_rect(above.id))
                {
                    regions = regions.iter().flat_map(|rect| subtract(*rect, overlay)).filter(Rect::is_positive).collect();
                }
            });
        }
        let regions = regions.into_iter().map(|rect| physical(rect, ctx.pixels_per_point())).collect();
        self.update_regions(ctx, regions);
    }
}

impl TitleBar {
    fn update_colors(&mut self, colors: [egui::Color32; 5]) {
        if self.colors == Some(colors) {
            return;
        }
        let result = self.bar.resolve().and_then(|bar| {
            let vtable = bar.vtable();
            for (color, setter) in [
                (colors[0], vtable.set_button_background),
                (colors[0], vtable.set_button_inactive_background),
                (colors[1], vtable.set_button_foreground),
                (colors[1], vtable.set_button_hover_foreground),
                (colors[1], vtable.set_button_pressed_foreground),
                (colors[2], vtable.set_button_inactive_foreground),
                (colors[3], vtable.set_button_hover_background),
                (colors[4], vtable.set_button_pressed_background),
            ] {
                let color: ColorReference = BoxedColor(Color { a: color.a(), r: color.r(), g: color.g(), b: color.b() }).into();
                // SAFETY: the setters accept IReference<Windows.UI.Color>, which owns the
                // boxed color for the duration of the call. Windows retains its own reference if needed.
                unsafe { setter(bar.as_raw(), color.as_raw()).ok()? };
            }
            Ok(())
        });
        if let Err(error) = result {
            log::warn!("Could not set the native caption colors: {error}");
        }
        self.colors = Some(colors);
    }

    fn update_regions(&mut self, ctx: &Context, mut regions: Vec<PhysicalRect>) {
        regions.retain(|rect| rect.width > 0 && rect.height > 0);
        regions.sort_unstable_by_key(|rect| (rect.x, rect.y, rect.width, rect.height));
        if self.regions.as_ref() == Some(&regions) {
            return;
        }
        let result = self.bar.resolve().and_then(|bar| {
            // SAFETY: the interface is resolved for this apartment and the slice is valid for
            // the duration of the call. WinRT takes the length in elements.
            unsafe { (bar.vtable().set_drag_rectangles)(bar.as_raw(), regions.len() as u32, regions.as_ptr()).ok() }
        });
        match result {
            Ok(()) => self.regions = Some(regions),
            Err(error) => self.restore_system_title_bar(ctx, &error),
        }
    }

    fn restore_system_title_bar(&mut self, ctx: &Context, error: &windows_core::Error) {
        log::warn!("Could not update the integrated Windows title bar: {error}");
        if let Ok(bar) = self.bar.resolve() {
            let _ = set_extended(&bar, false);
        }
        self.extended = false;
        self.failed = true;
        NativeTitleBar { integrated: false, left: 0.0, right: 0.0, height: 0.0 }.publish(ctx);
        ctx.request_repaint();
    }
}

fn caption_metrics(bar: &AppWindowTitleBar, integrated: bool, scale: f32) -> NativeTitleBar {
    let mut metrics = NativeTitleBar { integrated, left: 0.0, right: 0.0, height: 38.0 };
    if integrated {
        for (get, out) in
            [(bar.vtable().left_inset, &mut metrics.left), (bar.vtable().right_inset, &mut metrics.right), (bar.vtable().height, &mut metrics.height)]
        {
            let mut value = 0;
            // SAFETY: each getter takes a valid title-bar interface and a writable i32.
            if unsafe { get(bar.as_raw(), &mut value) }.is_ok() {
                *out = value.max(0) as f32 / scale.max(0.01);
            }
        }
    }
    metrics
}

fn set_extended(bar: &AppWindowTitleBar, extended: bool) -> Result<()> {
    // SAFETY: the WinRT interface came from AppWindow::TitleBar in the active COM apartment.
    unsafe { (bar.vtable().set_extended)(bar.as_raw(), extended).ok() }
}

fn physical(rect: Rect, scale: f32) -> PhysicalRect {
    let min = (rect.min.to_vec2() * scale).floor();
    let max = (rect.max.to_vec2() * scale).ceil();
    PhysicalRect { x: min.x as i32, y: min.y as i32, width: (max.x - min.x) as i32, height: (max.y - min.y) as i32 }
}

fn initialize_runtime() -> Result<()> {
    // The process-lifetime dependency keeps the framework registered for its WinRT objects.
    static INITIALIZED: OnceLock<HRESULT> = OnceLock::new();
    INITIALIZED.get_or_init(add_framework).ok()
}

fn add_framework() -> HRESULT {
    type CreateDependency = unsafe extern "system" fn(*mut c_void, *const u16, u64, i32, i32, *const u16, u32, *mut *mut u16) -> HRESULT;
    type AddDependency = unsafe extern "system" fn(*const u16, i32, u32, *mut *mut c_void, *mut *mut u16) -> HRESULT;
    // SAFETY: the two kernelbase exports are resolved dynamically so downlevel systems can
    // start normally. The signatures are from appmodel.h. The out pointer is freed with the
    // process heap as documented by TryCreatePackageDependency.
    unsafe {
        let kernel = GetModuleHandleW(w!("kernelbase.dll").as_ptr());
        if kernel.is_null() {
            return HRESULT(0x8007007e_u32 as i32);
        }
        let Some(create) = GetProcAddress(kernel, s!("TryCreatePackageDependency").as_ptr()) else {
            return HRESULT(0x8007007f_u32 as i32);
        };
        let Some(add) = GetProcAddress(kernel, s!("AddPackageDependency").as_ptr()) else {
            return HRESULT(0x8007007f_u32 as i32);
        };
        let create: CreateDependency = std::mem::transmute(create);
        let add: AddDependency = std::mem::transmute(add);
        let mut id = ptr::null_mut();
        let result = create(ptr::null_mut(), w!("Microsoft.WindowsAppRuntime.1.8_8wekyb3d8bbwe").as_ptr(), 0, 0, 0, ptr::null(), 0, &mut id);
        if result.is_err() {
            return result;
        }
        let mut context = ptr::null_mut();
        let result = add(id, 0, 0, &mut context, ptr::null_mut());
        HeapFree(GetProcessHeap(), 0, id.cast());
        result
    }
}

fn window_title_bar(id: u64) -> Result<AppWindowTitleBar> {
    let statics: AppWindowStatics = factory::<AppWindowRuntime, _>()?;
    let mut raw = ptr::null_mut();
    // SAFETY: WindowId represents the Win32 HWND value. Successful calls return owned WinRT
    // interface references. from_abi rejects null and takes ownership of each reference.
    unsafe {
        (statics.vtable().get_from_window_id)(statics.as_raw(), id, &mut raw).ok()?;
        let window: AppWindow = windows_core::Type::from_abi(raw)?;
        raw = ptr::null_mut();
        (window.vtable().get_title_bar)(window.as_raw(), &mut raw).ok()?;
        windows_core::Type::from_abi(raw)
    }
}

/// Split a drag rectangle around a visible popup, keeping each unobstructed part.
fn subtract(rect: Rect, overlay: Rect) -> [Rect; 4] {
    let overlap = rect.intersect(overlay);
    if !overlap.is_positive() {
        return [rect, Rect::NOTHING, Rect::NOTHING, Rect::NOTHING];
    }
    [
        Rect::from_x_y_ranges(rect.x_range(), rect.top()..=overlap.top()),
        Rect::from_x_y_ranges(rect.x_range(), overlap.bottom()..=rect.bottom()),
        Rect::from_x_y_ranges(rect.left()..=overlap.left(), overlap.y_range()),
        Rect::from_x_y_ranges(overlap.right()..=rect.right(), overlap.y_range()),
    ]
}
// Only the ABI members used above. GUIDs and vtable order come from Microsoft.UI.winmd
// in Windows App SDK 1.8. The omitted members preserve their pointer-sized slots.
type GetObject = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT;
type GetForWindow = unsafe extern "system" fn(*mut c_void, u64, *mut *mut c_void) -> HRESULT;
type GetMetric = unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT;
type SetColor = unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT;
#[repr(C)]
#[derive(PartialEq, Eq)]
struct PhysicalRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

struct AppWindowRuntime;
impl RuntimeName for AppWindowRuntime {
    const NAME: &'static str = "Microsoft.UI.Windowing.AppWindow";
}

windows_core::imp::define_interface!(AppWindowStatics, AppWindowStaticsVtable, 0x3c315c24_d540_5d72_b518_b226b83627cb);
#[repr(C)]
pub struct AppWindowStaticsVtable {
    base: IInspectable_Vtbl,
    create_overloads: [usize; 3],
    get_from_window_id: GetForWindow,
}

windows_core::imp::define_interface!(AppWindow, AppWindowVtable, 0xcfa788b3_643b_5c5e_ad4e_321d48a82acd);
#[repr(C)]
pub struct AppWindowVtable {
    base: IInspectable_Vtbl,
    properties_before_title_bar: [usize; 10],
    get_title_bar: GetObject,
}

windows_core::imp::define_interface!(AppWindowTitleBar, TitleBarVtable, 0x5574efa2_c91c_5700_a363_539c71a7aaf4);
#[repr(C)]
pub struct TitleBarVtable {
    base: IInspectable_Vtbl,
    background: [usize; 2],
    button_background: usize,
    set_button_background: SetColor,
    button_foreground: usize,
    set_button_foreground: SetColor,
    button_hover_background: usize,
    set_button_hover_background: SetColor,
    button_hover_foreground: usize,
    set_button_hover_foreground: SetColor,
    button_inactive_background: usize,
    set_button_inactive_background: SetColor,
    button_inactive_foreground: usize,
    set_button_inactive_foreground: SetColor,
    button_pressed_background: usize,
    set_button_pressed_background: SetColor,
    button_pressed_foreground: usize,
    set_button_pressed_foreground: SetColor,
    extends_content: usize,
    set_extended: unsafe extern "system" fn(*mut c_void, bool) -> HRESULT,
    foreground: [usize; 2],
    height: GetMetric,
    icon_and_inactive_colors: [usize; 6],
    left_inset: GetMetric,
    right_inset: GetMetric,
    reset_to_default: usize,
    set_drag_rectangles: unsafe extern "system" fn(*mut c_void, u32, *const PhysicalRect) -> HRESULT,
}

/// The Windows.UI.Color value carried by a nullable WinRT color property.
#[repr(C)]
#[derive(Clone, Copy)]
struct Color {
    a: u8,
    r: u8,
    g: u8,
    b: u8,
}

// The IID is the parameterized WinRT IID for IReference<Windows.UI.Color>.
windows_core::imp::define_interface!(ColorReference, ColorReferenceVtable, 0xab8e5d11_b0c1_5a21_95ae_f16bf3a37624);
windows_core::imp::interface_hierarchy!(ColorReference, IUnknown, IInspectable);
impl RuntimeName for ColorReference {
    const NAME: &'static str = "Windows.Foundation.IReference";
}

#[implement(ColorReference)]
struct BoxedColor(Color);

#[repr(C)]
pub struct ColorReferenceVtable {
    base: IInspectable_Vtbl,
    value: unsafe extern "system" fn(*mut c_void, *mut Color) -> HRESULT,
}

impl ColorReferenceVtable {
    const fn new<Identity: IUnknownImpl<Impl = BoxedColor>, const OFFSET: isize>() -> Self {
        unsafe extern "system" fn value<Identity: IUnknownImpl<Impl = BoxedColor>, const OFFSET: isize>(
            this: *mut c_void,
            out: *mut Color,
        ) -> HRESULT {
            if out.is_null() {
                return HRESULT(0x80004003_u32 as i32);
            }
            // SAFETY: windows-core installed this vtable for Identity at OFFSET. The caller
            // supplied a writable Windows.UI.Color output pointer.
            unsafe {
                let object = &*this.cast::<*mut c_void>().offset(OFFSET).cast::<Identity>();
                out.write(object.get_impl().0);
            }
            HRESULT(0)
        }
        Self { base: IInspectable_Vtbl::new::<Identity, ColorReference, OFFSET>(), value: value::<Identity, OFFSET> }
    }

    fn matches(iid: &GUID) -> bool {
        iid == &ColorReference::IID
    }
}
