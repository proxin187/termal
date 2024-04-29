use x11::xrender;
use x11::xlib;
use x11::xft;

use std::ffi;
use std::ptr;
use std::mem;

#[derive(Clone, Debug, Copy, PartialEq)]
pub struct Color {
    r: u64,
    g: u64,
    b: u64,
}

impl Color {
    pub fn new(r: u64, g: u64, b: u64) -> Color {
        Color {
            r,
            g,
            b,
        }
    }

    pub fn from_str(rgb: &str) -> Result<Color, Box<dyn std::error::Error>> {
        if !rgb.is_empty() {
            let rgb = rgb.split('-').collect::<Vec<&str>>();

            if rgb.len() == 3 {
                Ok(Color::new(u64::from_str_radix(rgb[0], 16)?, u64::from_str_radix(rgb[1], 16)?, u64::from_str_radix(rgb[2], 16)?))
            } else {
                Err("wrong rgb formatting".into())
            }
        } else {
            Ok(Color::new(0, 0, 0))
        }
    }

    pub fn encode(&self) -> u64 {
        self.b + (self.g << 8) + (self.r << 16)
    }

    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

pub struct Display {
    dpy: *mut xlib::_XDisplay,
    gc: *mut xlib::_XGC,
    xim: *mut xlib::_XIM,
    xic: *mut xlib::_XIC,
    draw: *mut x11::xft::XftDraw,

    back_buffer: u64,
    window: u64,
    screen: i32,
}

impl Drop for Display {
    fn drop(&mut self) {
        unsafe {
            xlib::XFreePixmap(self.dpy, self.back_buffer);
            xlib::XFreeGC(self.dpy, self.gc);
            xlib::XDestroyWindow(self.dpy, self.window);
            xlib::XCloseDisplay(self.dpy);
        }
    }
}

impl Display {
    pub fn open() -> Result<Display, Box<dyn std::error::Error>> {
        let dpy = unsafe { xlib::XOpenDisplay(ptr::null()) };

        if dpy.is_null() {
            Err("failed to open display".into())
        } else {
            unsafe {
                let bg = Color::new(0, 0, 0).encode();
                let window = xlib::XCreateSimpleWindow(
                    dpy,
                    xlib::XDefaultRootWindow(dpy),
                    0,
                    0,
                    500,
                    500,
                    0,
                    bg,
                    bg
                );

                let screen = xlib::XDefaultScreen(dpy);

                let mut values: xlib::XGCValues = mem::zeroed();

                let gc = xlib::XCreateGC(dpy, window, 0, &mut values);
                let back_buffer = xlib::XCreatePixmap(dpy, window, 945, 1020, 24);
                let draw = xft::XftDrawCreate(dpy, back_buffer, xlib::XDefaultVisual(dpy, screen), xlib::XDefaultColormap(dpy, screen));

                xlib::XSetLocaleModifiers("\0".as_ptr() as *const i8);

                let xim = xlib::XOpenIM(dpy, 0 as xlib::XrmDatabase, 0 as *mut ffi::c_char, 0 as *mut ffi::c_char);

                let xn_input_style = ffi::CString::new(xlib::XNInputStyle)?;
                let xn_client_window = ffi::CString::new(xlib::XNClientWindow)?;

                let xic = xlib::XCreateIC(
                    xim,
                    xn_input_style.as_ptr(), xlib::XIMPreeditNothing | xlib::XIMStatusNothing,
                    xn_client_window.as_ptr(), window as ffi::c_ulong,
                    ptr::null_mut::<ffi::c_void>()
                );

                xlib::XSync(dpy, xlib::False);

                Ok(Display {
                    dpy,
                    gc,
                    xim,
                    xic,
                    draw,
                    back_buffer,
                    window,
                    screen,
                })
            }
        }
    }

    pub fn resize_back_buffer(&mut self, window: &crate::terminal::Window) {
        unsafe {
            xlib::XFreePixmap(self.dpy, self.back_buffer);
            xft::XftDrawDestroy(self.draw);

            self.back_buffer = xlib::XCreatePixmap(self.dpy, self.window, window.width, window.height, 24);
            self.draw = xft::XftDrawCreate(self.dpy, self.back_buffer, xlib::XDefaultVisual(self.dpy, self.screen), xlib::XDefaultColormap(self.dpy, self.screen));
        }
    }

    pub fn get_window_attributes(&mut self) -> xlib::XWindowAttributes {
        unsafe {
            let mut attr: xlib::XWindowAttributes = mem::zeroed();

            xlib::XGetWindowAttributes(self.dpy, self.window, &mut attr);

            attr
        }
    }

    pub fn flush(&mut self) {
        unsafe {
            xlib::XFlush(self.dpy);
        }
    }

    pub fn map_window(&mut self) {
        unsafe {
            xlib::XMapWindow(self.dpy, self.window);
        }
    }

    pub fn lookup_string(&mut self, mut event: xlib::XKeyEvent) -> Result<String, Box<dyn std::error::Error>> {
        unsafe {
            let mut buf: [i8; 32] = [0; 32];
            let mut keysym = 0;

            xlib::Xutf8LookupString(self.xic, &mut event, buf.as_mut_ptr(), 32, &mut keysym, ptr::null_mut());

            Ok(String::from_utf8(buf.map(|x| x as u8).to_vec())?)
        }
    }

    fn null_terminate(&self, string: &str) -> String {
        format!("{}\0", string)
    }

    pub fn set_window_name(&mut self, name: &str) {
        unsafe {
            xlib::XStoreName(self.dpy, self.window, self.null_terminate(name).as_ptr() as *const i8);
        }
    }

    pub fn poll_event(&mut self) -> Option<Vec<xlib::XEvent>> {
        unsafe {
            let mut events: Vec<xlib::XEvent> = Vec::new();

            while xlib::XPending(self.dpy) > 0 {
                let mut event: xlib::XEvent = mem::zeroed();

                xlib::XNextEvent(self.dpy, &mut event);

                events.push(event);
            }

            if !events.is_empty() {
                Some(events)
            } else {
                None
            }
        }
    }

    pub fn keycode_to_keysym(&mut self, keycode: u8) -> u64 {
        unsafe {
            xlib::XKeycodeToKeysym(self.dpy, keycode, 0)
        }
    }

    pub fn select_input(&mut self) {
        unsafe {
            xlib::XSelectInput(self.dpy, self.window,
                                 xlib::KeyPressMask
                               | xlib::ExposureMask
                               | xlib::FocusChangeMask
                               | xlib::VisibilityChangeMask
                               | xlib::ButtonPressMask
                               | xlib::ButtonReleaseMask
                               | xlib::PointerMotionMask
            );
        }
    }

    pub fn define_cursor(&mut self) {
        unsafe {
            // https://tronche.com/gui/x/xlib/appendix/b/

            let cursor = xlib::XCreateFontCursor(self.dpy, 152);
            xlib::XDefineCursor(self.dpy, self.window, cursor);
        }
    }

    pub fn swap_buffers(&mut self, window: &crate::terminal::Window) {
        unsafe {
            xlib::XCopyArea(self.dpy, self.back_buffer, self.window, self.gc, 0, 0, window.width, window.height, 0, 0);
        }
    }

    pub fn xft_draw_string(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        height: u32,
        width: u32,
        font: *mut xft::XftFont,
        color: *const xft::XftColor,
    ) {
        unsafe {
            let rectangle = xlib::XRectangle {
                x: 0,
                y: 0,
                height: height as u16,
                width: width as u16,
            };

            xft::XftDrawSetClipRectangles(self.draw, x, y - 15, &rectangle, 1);

            xft::XftDrawStringUtf8(self.draw, color, font, x, y, self.null_terminate(text).as_ptr(), text.len() as i32);

            xft::XftDrawSetClip(self.draw, ptr::null_mut());
        }
    }

    pub fn xft_draw_string_32(
        &mut self,
        text: &[char],
        x: i32,
        y: i32,
        font: *mut xft::XftFont,
        color: *const xft::XftColor,
    ) {
        unsafe {
            xft::XftDrawString32(self.draw, color, font, x, y, [text, &['\0']].concat().as_ptr() as *const u32, text.len() as i32);
        }
    }

    pub fn xft_draw_glyph(
        &mut self,
        glyph: u32,
        x: i32,
        y: i32,
        font: *mut xft::XftFont,
        color: *const xft::XftColor,
    ) {
        unsafe {
            let specs = xft::XftGlyphFontSpec {
                font,
                glyph,
                x: x as i16,
                y: y as i16,
            };

            xft::XftDrawGlyphFontSpec(self.draw, color, &specs, 1);
        }
    }

    pub fn xft_measure_string(&self, text: &str, font: *mut xft::XftFont) -> xrender::_XGlyphInfo {
        unsafe {
            let mut extents: xrender::_XGlyphInfo = mem::zeroed();

            xft::XftTextExtentsUtf8(self.dpy, font, self.null_terminate(text).as_ptr(), text.len() as i32, &mut extents);

            extents
        }
    }

    pub fn xft_color_alloc_value(&self, rgb: Color) -> Result<xft::XftColor, Box<dyn std::error::Error>> {
        // convert 8bit rgb to 16bit rgb

        let xrender_color = x11::xrender::XRenderColor {
            red: rgb.r as u16 * 257,
            green: rgb.g as u16 * 257,
            blue: rgb.b as u16 * 257,
            alpha: 0xffff,
        };

        unsafe {
            let mut color: xft::XftColor = mem::zeroed();

            let result = xft::XftColorAllocValue(
                self.dpy,
                xlib::XDefaultVisual(self.dpy, self.screen),
                xlib::XDefaultColormap(self.dpy, self.screen),
                &xrender_color,
                &mut color,
            );

            if result == 0 {
                Err("XftColorAllocValue failed".into())
            } else {
                Ok(color)
            }
        }
    }

    pub fn load_font(&mut self, font: &str) -> Result<*mut xft::XftFont, Box<dyn std::error::Error>> {
        unsafe {
            let font = xft::XftFontOpenName(self.dpy, self.screen, self.null_terminate(font).as_ptr() as *const i8);

            if font.is_null() {
                Err("XftFontOpenName failed".into())
            } else {
                Ok(font)
            }

        }
    }

    pub fn outline_rec(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        unsafe {
            xlib::XSetForeground(self.dpy, self.gc, color.encode());
            xlib::XDrawRectangle(self.dpy, self.back_buffer, self.gc, x, y, width, height);
        }
    }

    pub fn draw_rec(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        unsafe {
            xlib::XSetForeground(self.dpy, self.gc, color.encode());
            xlib::XFillRectangle(self.dpy, self.back_buffer, self.gc, x, y, width, height);
        }
    }
}


