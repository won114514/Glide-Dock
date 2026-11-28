//! Direct2D execution of the shared frame commands.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use dock_core::{Color, Rect};
use dock_render::{DockFrame, DrawCommand};
use tiny_skia::{Pixmap, Transform};
use windows::Win32::Foundation::E_UNEXPECTED;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};
use windows::core::{Error, Result};
use windows_numerics::{Matrix3x2, Vector2};

pub(super) struct D2dPainter {
    context: ID2D1DeviceContext,
    shadow_context: ID2D1DeviceContext,
    target: Option<ID2D1Bitmap1>,
    factory: ID2D1Factory,
    solid: ID2D1SolidColorBrush,
    round_cap: ID2D1StrokeStyle,
    gradients: Vec<([Color; 2], ID2D1LinearGradientBrush)>,
    glow: Option<([u8; 3], ID2D1RadialGradientBrush)>,
    shadow: ID2D1Effect,
    shadow_source: Option<(D2D_SIZE_U, ID2D1Bitmap1)>,
    shadow_pending: bool,
    textures: HashMap<usize, Texture>,
    upload: Vec<u8>,
}

struct Texture {
    source: Weak<Pixmap>,
    bitmap: ID2D1Bitmap1,
}

impl D2dPainter {
    pub(super) fn new(dxgi: &IDXGIDevice) -> Result<Self> {
        unsafe {
            let device = D2D1CreateDevice(dxgi, None)?;
            let context = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let shadow_context = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            context.SetDpi(96.0, 96.0);
            shadow_context.SetDpi(96.0, 96.0);
            let solid = context.CreateSolidColorBrush(&color(Color::TRANSPARENT), None)?;
            let factory = context.GetFactory()?;
            let round_cap = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    ..Default::default()
                },
                None,
            )?;
            let shadow = context.CreateEffect(&CLSID_D2D1GaussianBlur)?;
            Ok(Self {
                context,
                shadow_context,
                target: None,
                factory,
                solid,
                round_cap,
                gradients: Vec::new(),
                glow: None,
                shadow,
                shadow_source: None,
                shadow_pending: false,
                textures: HashMap::new(),
                upload: Vec::new(),
            })
        }
    }

    pub(super) fn bind(&mut self, surface: &IDXGISurface) -> Result<()> {
        let properties =
            bitmap_properties(D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW);
        self.target = Some(unsafe {
            self.context
                .CreateBitmapFromDxgiSurface(surface, Some(&properties))
        }?);
        Ok(())
    }

    pub(super) fn unbind(&mut self) {
        unsafe { self.context.SetTarget(None::<&ID2D1Image>) };
        self.target = None;
    }

    pub(super) fn draw(&mut self, frame: &DockFrame) -> Result<()> {
        self.textures
            .retain(|_, texture| texture.source.strong_count() != 0);
        let context = self.context.clone();
        let target = self
            .target
            .as_ref()
            .ok_or_else(|| Error::from_hresult(E_UNEXPECTED))?;
        self.shadow_pending = false;
        unsafe {
            context.SetTarget(target);
            context.BeginDraw();
            context.SetTransform(&Matrix3x2::identity());
            context.Clear(None);
        }
        let result = frame
            .commands
            .iter()
            .try_for_each(|command| self.draw_command(command));
        // End the batch and release the context binding even if an individual
        // command failed. The cached bitmap is released separately on resize.
        let end = unsafe { context.EndDraw(None, None) };
        unsafe {
            context.SetTarget(None::<&ID2D1Image>);
            self.shadow.SetInput(0, None::<&ID2D1Image>, true);
        }
        result.and(end)
    }

    fn draw_command(&mut self, command: &DrawCommand) -> Result<()> {
        unsafe {
            match *command {
                DrawCommand::Fill {
                    rect,
                    radius,
                    color: fill,
                } => {
                    self.solid.SetColor(&color(fill));
                    self.context
                        .FillRoundedRectangle(&round_rect(rect, radius), &self.solid);
                }
                DrawCommand::Vertical {
                    rect,
                    radius,
                    top,
                    bottom,
                } => {
                    let brush = self.gradient(top, bottom)?;
                    brush.SetStartPoint(Vector2::new(rect.x, rect.y));
                    brush.SetEndPoint(Vector2::new(rect.x, rect.bottom()));
                    self.context
                        .FillRoundedRectangle(&round_rect(rect, radius), &brush);
                }
                DrawCommand::Stroke {
                    rect,
                    radius,
                    width,
                    color: stroke,
                } => {
                    self.solid.SetColor(&color(stroke));
                    self.context.DrawRoundedRectangle(
                        &round_rect(rect, radius),
                        &self.solid,
                        width,
                        None::<&ID2D1StrokeStyle>,
                    );
                }
                DrawCommand::Line {
                    from,
                    to,
                    width,
                    color: stroke,
                } => {
                    self.solid.SetColor(&color(stroke));
                    self.context.DrawLine(
                        point(from),
                        point(to),
                        &self.solid,
                        width,
                        &self.round_cap,
                    );
                }
                DrawCommand::Circle {
                    center,
                    radius,
                    stroke,
                    color: fill,
                } => {
                    self.solid.SetColor(&color(fill));
                    let ellipse = D2D1_ELLIPSE {
                        point: point(center),
                        radiusX: radius,
                        radiusY: radius,
                    };
                    if let Some(width) = stroke {
                        self.context.DrawEllipse(
                            &ellipse,
                            &self.solid,
                            width,
                            None::<&ID2D1StrokeStyle>,
                        );
                    } else {
                        self.context.FillEllipse(&ellipse, &self.solid);
                    }
                }
                DrawCommand::Polygon {
                    ref points,
                    color: fill,
                } => {
                    self.polygon(points, fill)?;
                }
                DrawCommand::Glow {
                    rect,
                    spread,
                    color: fill,
                    strength,
                } => {
                    self.glow(rect, spread, fill, strength)?;
                }
                DrawCommand::Shadow {
                    rect,
                    radius,
                    blur,
                    offset,
                    color: fill,
                    ..
                } => {
                    self.shadow(rect.translate(0.0, offset), radius, blur, fill)?;
                }
                DrawCommand::Bitmap {
                    ref pixels,
                    transform,
                    opacity,
                } => {
                    self.bitmap(pixels, transform, opacity)?;
                }
            }
        }
        Ok(())
    }

    fn gradient(&mut self, top: Color, bottom: Color) -> Result<ID2D1LinearGradientBrush> {
        let key = [top, bottom];
        if let Some((_, brush)) = self.gradients.iter().find(|(colors, _)| *colors == key) {
            return Ok(brush.clone());
        }
        let stops = [gradient_stop(0.0, top), gradient_stop(1.0, bottom)];
        let brush = unsafe {
            let stops = gradient_stops(&self.context, &stops)?;
            self.context.CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: Vector2::new(0.0, 0.0),
                    endPoint: Vector2::new(0.0, 1.0),
                },
                None,
                &stops,
            )?
        };
        if self.gradients.len() >= 8 {
            self.gradients.remove(0);
        }
        self.gradients.push((key, brush.clone()));
        Ok(brush)
    }

    fn polygon(&self, points: &[(f32, f32)], fill: Color) -> Result<()> {
        let Some((&first, rest)) = points.split_first() else {
            return Ok(());
        };
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(point(first), D2D1_FIGURE_BEGIN_FILLED);
            for &vertex in rest {
                sink.AddLine(point(vertex));
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            self.solid.SetColor(&color(fill));
            self.context
                .FillGeometry(&geometry, &self.solid, None::<&ID2D1Brush>);
        }
        Ok(())
    }

    fn glow(&mut self, rect: Rect, spread: f32, fill: Color, strength: f32) -> Result<()> {
        let rgb = [fill.r, fill.g, fill.b];
        let brush = match &self.glow {
            Some((key, brush)) if *key == rgb => brush.clone(),
            _ => {
                // Equivalent to the CPU gradient with its transparent inner
                // radius at 30% and peak at 55% of the remaining interval.
                let transparent = fill.with_alpha(0);
                let stops = [
                    gradient_stop(0.0, transparent),
                    gradient_stop(0.30, transparent),
                    gradient_stop(0.685, fill.with_alpha(255)),
                    gradient_stop(1.0, transparent),
                ];
                let brush = unsafe {
                    let stops = gradient_stops(&self.context, &stops)?;
                    self.context.CreateRadialGradientBrush(
                        &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                            center: Vector2::new(0.0, 0.0),
                            gradientOriginOffset: Vector2::new(0.0, 0.0),
                            radiusX: 1.0,
                            radiusY: 1.0,
                        },
                        None,
                        &stops,
                    )?
                };
                self.glow = Some((rgb, brush.clone()));
                brush
            }
        };
        let outer = (rect.width.max(rect.height) / 2.0 + spread).max(1.0);
        unsafe {
            brush.SetCenter(Vector2::new(rect.center().x, rect.center().y));
            brush.SetRadiusX(outer);
            brush.SetRadiusY(outer);
            brush.SetOpacity(0.55 * strength.clamp(0.0, 1.0) * (f32::from(fill.a) / 255.0));
            self.context
                .FillRectangle(&d2d_rect(rect.inset(-spread)), &brush);
        }
        Ok(())
    }

    fn shadow(&mut self, rect: Rect, radius: f32, blur: f32, fill: Color) -> Result<()> {
        // A compact GPU scratch bitmap keeps gaussian blur off the large
        // transparent headroom reserved for the menu. Reuse its allocation.
        if self.shadow_pending {
            // Finish the previous use before changing the reusable input.
            unsafe { self.context.Flush(None, None) }?;
        }
        let size = D2D_SIZE_U {
            width: (rect.width.ceil() as u32 + 2).next_multiple_of(64),
            height: (rect.height.ceil() as u32 + 2).next_multiple_of(64),
        };
        let source = match &self.shadow_source {
            Some((cached, source))
                if cached.width >= size.width && cached.height >= size.height =>
            {
                source.clone()
            }
            _ => {
                let properties = bitmap_properties(D2D1_BITMAP_OPTIONS_TARGET);
                let source =
                    unsafe { self.shadow_context.CreateBitmap(size, None, 0, &properties) }?;
                self.shadow_source = Some((size, source.clone()));
                source
            }
        };
        let brush = &self.solid;
        let target = &self.shadow_context;
        let absolute = Vector2::new(rect.x.floor() - 1.0, rect.y.floor() - 1.0);
        let local = rect.translate(-absolute.X, -absolute.Y);
        let sigma = blur.max(1.0) / 2.0;
        unsafe {
            target.SetTarget(&source);
            target.BeginDraw();
            target.Clear(None);
            brush.SetColor(&color(fill));
            target.FillRoundedRectangle(&round_rect(local, radius), brush);
            let result = target.EndDraw(None, None);
            target.SetTarget(None::<&ID2D1Image>);
            result?;

            self.shadow.SetInput(0, &source, true);
            self.shadow.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &sigma.to_ne_bytes(),
            )?;
            let output = self.shadow.GetOutput()?;
            let bounds = self.context.GetImageLocalBounds(&output)?;
            let origin = Vector2::new(absolute.X + bounds.left, absolute.Y + bounds.top);
            self.context.DrawImage(
                &output,
                Some(&origin),
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
        }
        self.shadow_pending = true;
        Ok(())
    }

    fn bitmap(&mut self, pixels: &Arc<Pixmap>, transform: Transform, opacity: f32) -> Result<()> {
        if opacity <= 0.001 {
            return Ok(());
        }
        let key = Arc::as_ptr(pixels) as usize;
        let bitmap = match self.textures.get(&key) {
            Some(texture) => texture.bitmap.clone(),
            None => {
                self.upload.clear();
                self.upload.reserve(pixels.data().len());
                for &[r, g, b, a] in pixels.data().as_chunks::<4>().0 {
                    self.upload.extend_from_slice(&[b, g, r, a]);
                }
                let properties = bitmap_properties(D2D1_BITMAP_OPTIONS_NONE);
                let bitmap = unsafe {
                    self.context.CreateBitmap(
                        D2D_SIZE_U {
                            width: pixels.width(),
                            height: pixels.height(),
                        },
                        Some(self.upload.as_ptr().cast()),
                        pixels.width() * 4,
                        &properties,
                    )?
                };
                self.textures.insert(
                    key,
                    Texture {
                        source: Arc::downgrade(pixels),
                        bitmap: bitmap.clone(),
                    },
                );
                if self.upload.capacity() > 65_536 {
                    self.upload = Vec::new();
                } else {
                    self.upload.clear();
                }
                bitmap
            }
        };
        let matrix = Matrix3x2 {
            M11: transform.sx,
            M12: transform.ky,
            M21: transform.kx,
            M22: transform.sy,
            M31: transform.tx,
            M32: transform.ty,
        };
        unsafe {
            self.context.SetTransform(&matrix);
            let interpolation = if transform.is_translate() {
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR
            } else {
                D2D1_INTERPOLATION_MODE_LINEAR
            };
            self.context
                .DrawBitmap(&bitmap, None, opacity, interpolation, None, None);
            self.context.SetTransform(&Matrix3x2::identity());
        }
        Ok(())
    }
}

fn gradient_stops(
    context: &ID2D1DeviceContext,
    stops: &[D2D1_GRADIENT_STOP],
) -> Result<ID2D1GradientStopCollection1> {
    unsafe {
        context.CreateGradientStopCollection(
            stops,
            D2D1_COLOR_SPACE_SRGB,
            D2D1_COLOR_SPACE_SRGB,
            D2D1_BUFFER_PRECISION_8BPC_UNORM,
            D2D1_EXTEND_MODE_CLAMP,
            D2D1_COLOR_INTERPOLATION_MODE_PREMULTIPLIED,
        )
    }
}

fn gradient_stop(position: f32, fill: Color) -> D2D1_GRADIENT_STOP {
    D2D1_GRADIENT_STOP {
        position,
        color: color(fill),
    }
}

fn bitmap_properties(bitmap_options: D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: bitmap_options,
        ..Default::default()
    }
}

fn point(value: (f32, f32)) -> Vector2 {
    Vector2::new(value.0, value.1)
}

fn color(value: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(value.r) / 255.0,
        g: f32::from(value.g) / 255.0,
        b: f32::from(value.b) / 255.0,
        a: f32::from(value.a) / 255.0,
    }
}

fn d2d_rect(rect: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left(),
        right: rect.right(),
        top: rect.top(),
        bottom: rect.bottom(),
    }
}

fn round_rect(rect: Rect, radius: f32) -> D2D1_ROUNDED_RECT {
    let radius = radius.min(rect.width.min(rect.height) / 2.0).max(0.0);
    D2D1_ROUNDED_RECT {
        rect: d2d_rect(rect),
        radiusX: radius,
        radiusY: radius,
    }
}
