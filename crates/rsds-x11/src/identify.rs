//! Identify: a number badge on each output, drawn as an override-redirect
//! window whose background is a pixmap holding the badge, so the server
//! repaints it without our help.

use rsds_core::{
    Error, Output,
    badge::{self, Badge, MARGIN},
    layout::Rect,
};
use x11rb::{
    connection::Connection,
    protocol::xproto::{self, ConnectionExt as _},
    rust_connection::RustConnection,
};

use crate::lost;

#[derive(Default)]
pub(crate) struct Identify {
    windows: Vec<xproto::Window>,
}

impl Identify {
    pub(crate) fn show(
        &mut self,
        conn: &RustConnection,
        root: xproto::Window,
        outputs: &[Output],
        labels: &[(String, usize)],
    ) -> Result<(), Error> {
        self.hide(conn);
        let depth = conn
            .setup()
            .roots
            .iter()
            .find(|s| s.root == root)
            .map_or(24, |s| s.root_depth);
        for (name, number) in labels {
            let Some(area) = outputs.iter().find(|o| &o.name == name).and_then(|o| o.config().rect()) else {
                continue;
            };
            let window = draw(conn, root, depth, area, &Badge::new(*number)).map_err(lost)?;
            self.windows.push(window);
        }
        conn.flush().map_err(lost)
    }

    pub(crate) fn hide(&mut self, conn: &RustConnection) {
        for w in self.windows.drain(..) {
            let _ = conn.destroy_window(w);
        }
        let _ = conn.flush();
    }
}

fn draw(
    conn: &RustConnection,
    root: xproto::Window,
    depth: u8,
    area: Rect,
    b: &Badge,
) -> Result<xproto::Window, x11rb::errors::ReplyOrIdError> {
    let (w, h) = (b.width as u16, b.height as u16);
    let pixmap = conn.generate_id()?;
    conn.create_pixmap(depth, pixmap, root, w, h)?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, pixmap, &xproto::CreateGCAux::new())?;
    let fill = |colour: u32, rects: &[Rect]| -> Result<(), x11rb::errors::ConnectionError> {
        // TrueColor: the pixel value is the RGB.
        conn.change_gc(gc, &xproto::ChangeGCAux::new().foreground(colour & 0xff_ffff))?;
        let rects: Vec<xproto::Rectangle> = rects
            .iter()
            .map(|r| xproto::Rectangle { x: r.x as i16, y: r.y as i16, width: r.w as u16, height: r.h as u16 })
            .collect();
        conn.poly_fill_rectangle(pixmap, gc, &rects)?;
        Ok(())
    };
    fill(badge::BACKGROUND, &[Rect::new(0, 0, b.width, b.height)])?;
    fill(badge::BORDER, &b.border)?;
    fill(badge::DIGIT, &b.segments)?;
    conn.free_gc(gc)?;

    let window = conn.generate_id()?;
    let x = area.x + MARGIN;
    let y = area.bottom() - MARGIN - b.height;
    conn.create_window(
        x11rb::COPY_FROM_PARENT as u8,
        window,
        root,
        x as i16,
        y as i16,
        w,
        h,
        0,
        xproto::WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &xproto::CreateWindowAux::new().override_redirect(1).background_pixmap(pixmap),
    )?;
    // The window keeps its background.
    conn.free_pixmap(pixmap)?;
    conn.map_window(window)?;
    Ok(window)
}
