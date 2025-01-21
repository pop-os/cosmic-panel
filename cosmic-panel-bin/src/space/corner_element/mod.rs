use std::cell::RefCell;

use smithay::{
    backend::renderer::{
        element::{Element, Kind, RenderElement, UnderlyingStorage},
        gles::{
            element::PixelShaderElement,
            ffi::{BLEND, FUNC_ADD, SRC_ALPHA, ZERO},
            GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, Uniform, UniformName,
            UniformType,
        },
    },
    utils::{Buffer, Logical, Physical, Rectangle},
};

pub static RECTANGLE_SHADER: &str = include_str!("./shader.frag");

pub struct RoundedRectangleShader(pub GlesPixelProgram);

#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct RoundedRectangleSettings {
    pub rad_tl: f32,
    pub rad_tr: f32,
    pub rad_bl: f32,
    pub rad_br: f32,
    pub loc: [f32; 2],
    pub rect_size: [f32; 2],
    pub border_width: f32,
    pub drop_shadow: f32,
    pub bg_color: [f32; 4],
    pub border_color: [f32; 4],
}

pub struct RoundedRectangleShaderElement(PixelShaderElement);

impl RoundedRectangleShader {
    pub fn get(renderer: &GlesRenderer) -> GlesPixelProgram {
        renderer
            .egl_context()
            .user_data()
            .get::<RoundedRectangleShader>()
            .expect("Custom Shaders not initialized")
            .0
            .clone()
    }

    pub fn element(
        renderer: &GlesRenderer,
        geo: Rectangle<i32, Logical>,
        settings: RoundedRectangleSettings,
    ) -> RoundedRectangleShaderElement {
        let user_data = renderer.egl_context().user_data();
        user_data.insert_if_missing(|| {
            RefCell::new(None::<(RoundedRectangleSettings, PixelShaderElement)>)
        });
        let mut cache = user_data
            .get::<RefCell<Option<(RoundedRectangleSettings, PixelShaderElement)>>>()
            .unwrap()
            .borrow_mut();

        let elem = cache.take().filter(|(s, _)| *s == settings).unwrap_or_else(|| {
            let shader = Self::get(renderer);
            (
                settings,
                PixelShaderElement::new(
                    shader,
                    geo,
                    None, // TODO
                    1.0,
                    vec![
                        Uniform::new("rad_tl", settings.rad_tl),
                        Uniform::new("rad_tr", settings.rad_tr),
                        Uniform::new("rad_bl", settings.rad_bl),
                        Uniform::new("rad_br", settings.rad_br),
                        Uniform::new("loc", settings.loc),
                        Uniform::new("rect_size", settings.rect_size),
                        Uniform::new("border_width", settings.border_width),
                        Uniform::new("drop_shadow", settings.drop_shadow),
                        Uniform::new("bg_color", settings.bg_color),
                        Uniform::new("border_color", settings.border_color),
                    ],
                    Kind::Unspecified,
                ),
            )
        });
        *cache = Some(elem);

        let elem = &mut cache.as_mut().unwrap().1;
        if elem.geometry(1.0.into()).to_logical(1) != geo {
            elem.resize(geo, None);
        }
        RoundedRectangleShaderElement(elem.clone())
    }
}

pub fn init_shaders(gles_renderer: &mut GlesRenderer) -> Result<(), GlesError> {
    {
        let egl_context = gles_renderer.egl_context();
        if egl_context.user_data().get::<RoundedRectangleShader>().is_some() {
            return Ok(());
        }
    }

    let rectangle_shader = gles_renderer.compile_custom_pixel_shader(RECTANGLE_SHADER, &[
        UniformName::new("rad_tl", UniformType::_1f),
        UniformName::new("rad_tr", UniformType::_1f),
        UniformName::new("rad_bl", UniformType::_1f),
        UniformName::new("rad_br", UniformType::_1f),
        UniformName::new("loc", UniformType::_2f),
        UniformName::new("rect_size", UniformType::_2f),
        UniformName::new("border_width", UniformType::_1f),
        UniformName::new("drop_shadow", UniformType::_1f),
        UniformName::new("bg_color", UniformType::_4f),
        UniformName::new("border_color", UniformType::_4f),
    ])?;

    let egl_context = gles_renderer.egl_context();
    egl_context.user_data().insert_if_missing(|| RoundedRectangleShader(rectangle_shader));

    Ok(())
}

impl Element for RoundedRectangleShaderElement {
    fn id(&self) -> &smithay::backend::renderer::element::Id {
        self.0.id()
    }

    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        self.0.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.0.src()
    }

    fn geometry(&self, scale: smithay::utils::Scale<f64>) -> Rectangle<i32, Physical> {
        self.0.geometry(scale)
    }
}

impl RenderElement<GlesRenderer> for RoundedRectangleShaderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[smithay::utils::Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        _ = frame.with_context(|gl| unsafe {
            gl.Enable(BLEND);
            gl.BlendFuncSeparate(ZERO, SRC_ALPHA, ZERO, SRC_ALPHA);
            gl.BlendEquation(FUNC_ADD);
        });
        let res = self.0.draw(frame, src, dst, damage, opaque_regions);
        _ = frame.with_context(|gl| unsafe {
            gl.Disable(BLEND);
        });
        res
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage> {
        self.0.underlying_storage(renderer)
    }
}
