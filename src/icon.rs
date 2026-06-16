use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_app_kit::{
    NSBezierPath, NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSColor,
    NSImage, NSLineCapStyle, NSLineJoinStyle,
};
use objc2_foundation::{MainThreadMarker, NSDictionary, NSPoint, NSRect, NSSize, NSString};

const STATUS_BAR_ICON_SIZE: f64 = 18.0;
const APPLICATION_ICON_SIZE: f64 = 256.0;
const ICON_VIEWBOX_SIZE: f64 = 24.0;
const APPLICATION_ICONSET_PNGS: [(&str, f64); 10] = [
    ("icon_16x16.png", 16.0),
    ("icon_16x16@2x.png", 32.0),
    ("icon_32x32.png", 32.0),
    ("icon_32x32@2x.png", 64.0),
    ("icon_128x128.png", 128.0),
    ("icon_128x128@2x.png", 256.0),
    ("icon_256x256.png", 256.0),
    ("icon_256x256@2x.png", 512.0),
    ("icon_512x512.png", 512.0),
    ("icon_512x512@2x.png", 1024.0),
];

// Status bar icon sizes and viewbox settings

pub fn make_application_icon(mtm: MainThreadMarker) -> Retained<NSImage> {
    make_microphone_icon(
        mtm,
        NSSize::new(APPLICATION_ICON_SIZE, APPLICATION_ICON_SIZE),
        &NSColor::blackColor(),
        false,
    )
}

pub fn make_status_bar_icon(mtm: MainThreadMarker) -> Retained<NSImage> {
    make_microphone_icon(
        mtm,
        NSSize::new(STATUS_BAR_ICON_SIZE, STATUS_BAR_ICON_SIZE),
        &NSColor::blackColor(),
        true,
    )
}

pub fn write_application_iconset(output_dir: &Path) -> Result<(), String> {
    let mtm = MainThreadMarker::new().expect("must run on main thread");
    std::fs::create_dir_all(output_dir).map_err(|error| {
        format!(
            "failed to create iconset directory {}: {}",
            output_dir.display(),
            error
        )
    })?;

    for (file_name, icon_size) in APPLICATION_ICONSET_PNGS {
        let output_path = output_dir.join(file_name);
        write_png_icon(mtm, icon_size, &output_path)?;
    }

    Ok(())
}

#[allow(deprecated)]
pub fn make_status_bar_active_icon(_mtm: MainThreadMarker) -> Retained<NSImage> {
    let image = NSImage::initWithSize(
        NSImage::alloc(),
        NSSize::new(STATUS_BAR_ICON_SIZE, STATUS_BAR_ICON_SIZE),
    );
    image.lockFocus();

    let size = image.size();
    let circle = NSBezierPath::bezierPath();
    circle.setLineWidth(scaled_value(size, 1.5));
    circle.appendBezierPathWithOvalInRect(scaled_svg_rect(size, 2.0, 2.0, 20.0, 20.0));
    circle.stroke();

    draw_microphone_symbol(size);

    image.unlockFocus();
    image.setTemplate(true);
    image
}

#[allow(deprecated)]
fn make_microphone_icon(
    _mtm: MainThreadMarker,
    size: NSSize,
    color: &NSColor,
    template: bool,
) -> Retained<NSImage> {
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    image.lockFocus();

    color.set();
    draw_microphone_symbol(size);

    image.unlockFocus();
    image.setTemplate(template);
    image
}

fn write_png_icon(mtm: MainThreadMarker, icon_size: f64, output_path: &Path) -> Result<(), String> {
    let image = make_microphone_icon(
        mtm,
        NSSize::new(icon_size, icon_size),
        &NSColor::blackColor(),
        false,
    );
    let tiff_data = image.TIFFRepresentation().ok_or_else(|| {
        format!(
            "failed to produce TIFF representation for {}px application icon",
            icon_size
        )
    })?;
    let bitmap_image = NSBitmapImageRep::imageRepWithData(&tiff_data)
        .ok_or_else(|| format!("failed to decode TIFF bitmap for {}", output_path.display()))?;
    let properties: Retained<NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>> =
        NSDictionary::new();
    let png_data = unsafe {
        bitmap_image.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
    }
    .ok_or_else(|| format!("failed to encode PNG data for {}", output_path.display()))?;
    let output_path_string = output_path.to_str().ok_or_else(|| {
        format!(
            "application icon path is not valid UTF-8: {}",
            output_path.display()
        )
    })?;
    let ns_output_path = NSString::from_str(output_path_string);

    if !png_data.writeToFile_atomically(&ns_output_path, true) {
        return Err(format!(
            "failed to write application icon PNG {}",
            output_path.display()
        ));
    }

    Ok(())
}

fn draw_microphone_symbol(size: NSSize) {
    let capsule = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        scaled_svg_rect(size, 8.0, 2.0, 8.0, 14.0),
        scaled_value(size, 4.0),
        scaled_value(size, 4.0),
    );
    capsule.fill();

    let microphone_frame = NSBezierPath::bezierPath();
    microphone_frame.setLineWidth(scaled_value(size, 2.0));
    microphone_frame.setLineCapStyle(NSLineCapStyle::Round);
    microphone_frame.setLineJoinStyle(NSLineJoinStyle::Round);
    microphone_frame.moveToPoint(scaled_svg_point(size, 6.0, 10.0));
    microphone_frame.lineToPoint(scaled_svg_point(size, 6.0, 12.0));
    microphone_frame.curveToPoint_controlPoint1_controlPoint2(
        scaled_svg_point(size, 12.0, 18.0),
        scaled_svg_point(size, 6.0, 15.3137),
        scaled_svg_point(size, 8.6863, 18.0),
    );
    microphone_frame.curveToPoint_controlPoint1_controlPoint2(
        scaled_svg_point(size, 18.0, 12.0),
        scaled_svg_point(size, 15.3137, 18.0),
        scaled_svg_point(size, 18.0, 15.3137),
    );
    microphone_frame.lineToPoint(scaled_svg_point(size, 18.0, 10.0));
    microphone_frame.stroke();

    let stem = NSBezierPath::bezierPath();
    stem.setLineWidth(scaled_value(size, 2.0));
    stem.setLineCapStyle(NSLineCapStyle::Round);
    stem.moveToPoint(scaled_svg_point(size, 12.0, 18.0));
    stem.lineToPoint(scaled_svg_point(size, 12.0, 20.0));
    stem.stroke();

    let base = NSBezierPath::bezierPath();
    base.setLineWidth(scaled_value(size, 2.0));
    base.setLineCapStyle(NSLineCapStyle::Round);
    base.moveToPoint(scaled_svg_point(size, 9.0, 21.0));
    base.lineToPoint(scaled_svg_point(size, 15.0, 21.0));
    base.stroke();
}

fn scaled_svg_point(size: NSSize, x: f64, y: f64) -> NSPoint {
    let scale = icon_scale(size);
    NSPoint::new(x * scale, size.height - (y * scale))
}

fn scaled_svg_rect(size: NSSize, x: f64, y: f64, width: f64, height: f64) -> NSRect {
    let scale = icon_scale(size);
    NSRect::new(
        NSPoint::new(x * scale, size.height - ((y + height) * scale)),
        NSSize::new(width * scale, height * scale),
    )
}

fn scaled_value(size: NSSize, value: f64) -> f64 {
    value * icon_scale(size)
}

fn icon_scale(size: NSSize) -> f64 {
    size.width.min(size.height) / ICON_VIEWBOX_SIZE
}
