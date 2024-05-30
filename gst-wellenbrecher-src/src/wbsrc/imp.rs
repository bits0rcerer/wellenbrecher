use std::cmp::min;
use std::path::PathBuf;
use std::sync::Mutex;

use gst::glib;
use gst::glib::bitflags::Flags;
use gst::prelude::{ElementExt, ParamSpecBuilderExt, ToValue};
use gst::subclass::prelude::{
    ElementImpl, GstObjectImpl, ObjectImpl, ObjectImplExt, ObjectSubclass, ObjectSubclassExt,
};
use gst_base::prelude::BaseSrcExt;
use gst_base::subclass::prelude::{BaseSrcImpl, BaseSrcImplExt};

use once_cell::sync::Lazy;
use wellenbrecher_canvas::{Bgra, Canvas};

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        "wbsrc",
        gst::DebugColorFlags::empty(),
        Some("Wellenbrecher canvas source"),
    )
});

#[derive(Debug, Clone)]
struct Settings {
    width: u32,
    height: u32,
    flink: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            flink: PathBuf::from("/tmp/wellenbrecher-canvas"),
        }
    }
}

#[derive(Debug, Default)]
struct State {
    canvas: Option<Canvas>,
}

#[derive(Default)]
pub struct WellenbrecherSrc {
    settings: Mutex<Settings>,
    state: Mutex<State>,
}

impl WellenbrecherSrc {}

#[glib::object_subclass]
impl ObjectSubclass for WellenbrecherSrc {
    const NAME: &'static str = "WellenbrecherSrc";
    type Type = super::WellenbrecherSrc;
    type ParentType = gst_base::BaseSrc;
}

impl ObjectImpl for WellenbrecherSrc {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
            vec![
                glib::ParamSpecString::builder("flink")
                    .nick("Shared memory file link")
                    .blurb("Shared memory file link")
                    .default_value("/tmp/wellenbrecher-canvas")
                    .build(),
                glib::ParamSpecUInt::builder("width")
                    .nick("Canvas width")
                    .blurb("Width of the wellenbrecher canvas")
                    .minimum(1)
                    .default_value(1280)
                    .build(),
                glib::ParamSpecUInt::builder("height")
                    .nick("Canvas height")
                    .blurb("Height of the wellenbrecher canvas")
                    .minimum(1)
                    .default_value(720)
                    .build(),
            ]
        });

        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        match pspec.name() {
            "flink" => {
                let mut settings = self.settings.lock().unwrap();
                let flink: String = value.get().expect("type checked upstream");
                gst::info!(
                    CAT,
                    imp: self,
                    "Changing canvas file link from {} to {}",
                    settings.flink.to_string_lossy(),
                    flink
                );
                settings.flink = PathBuf::from(flink);
                drop(settings);

                let _ = self
                    .obj()
                    .post_message(gst::message::Latency::builder().src(&*self.obj()).build());
            }
            "width" => {
                let mut settings = self.settings.lock().unwrap();
                let width = value.get().expect("type checked upstream");
                gst::info!(
                    CAT,
                    imp: self,
                    "Changing width from {} to {}",
                    settings.width,
                    width
                );
                settings.width = width;
            }
            "height" => {
                let mut settings = self.settings.lock().unwrap();
                let height = value.get().expect("type checked upstream");
                gst::info!(
                    CAT,
                    imp: self,
                    "Changing height from {} to {}",
                    settings.height,
                    height
                );
                settings.height = height;
            }
            _ => unimplemented!(),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        match pspec.name() {
            "flink" => {
                let settings = self.settings.lock().unwrap();
                settings.flink.to_string_lossy().to_value()
            }
            "width" => {
                let settings = self.settings.lock().unwrap();
                settings.width.to_value()
            }
            "height" => {
                let settings = self.settings.lock().unwrap();
                settings.height.to_value()
            }
            _ => unimplemented!(),
        }
    }

    fn constructed(&self) {
        self.parent_constructed();
        let obj = self.obj();
        obj.set_live(true);
        obj.set_format(gst::Format::Time);
    }
}

impl GstObjectImpl for WellenbrecherSrc {}

impl ElementImpl for WellenbrecherSrc {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                "Wellenbrecher canvas source",
                "Source",
                "Allows to use the wellenbrecher canvas as video source",
                "bits0rcerer https://github.com/bits0rcerer",
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
            let caps = gst_video::VideoCapsBuilder::new()
                .format_list([gst_video::VideoFormat::Bgra])
                .build();
            let src_pad_template = gst::PadTemplate::new(
                "src",
                gst::PadDirection::Src,
                gst::PadPresence::Always,
                &caps,
            )
            .unwrap();

            vec![src_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }
}

impl BaseSrcImpl for WellenbrecherSrc {
    fn set_caps(&self, caps: &gst::Caps) -> Result<(), gst::LoggableError> {
        let info = gst_video::VideoInfo::from_caps(caps).map_err(|_| {
            gst::loggable_error!(CAT, "Failed to build `VideoInfo` from caps {}", caps)
        })?;

        gst::debug!(CAT, imp: self, "Configuring for caps {}", caps);

        self.obj()
            .set_blocksize(info.width() * info.height() * std::mem::size_of::<Bgra>() as u32);

        let _ = self
            .obj()
            .post_message(gst::message::Latency::builder().src(&*self.obj()).build());

        Ok(())
    }

    fn fixate(&self, mut caps: gst::Caps) -> gst::Caps {
        caps.truncate();
        {
            let caps = caps.make_mut();
            let s = caps.structure_mut(0).unwrap();

            let settings = self.settings.lock().unwrap();

            s.fixate_field_nearest_int("width", settings.width as i32);
            s.fixate_field_nearest_int("height", settings.height as i32);
        }

        self.parent_fixate(caps)
    }

    fn start(&self) -> Result<(), gst::ErrorMessage> {
        let settings = self.settings.lock().unwrap();
        let mut state = self.state.lock().unwrap();

        if state.canvas.is_none() {
            let canvas = Canvas::open(settings.flink.as_path(), false, None)
                .expect("unable to open shared memory");

            if canvas.width() != settings.width || canvas.width() != settings.height {
                panic!("specified canvas dimensions ({}x{}) do not match shared canvas dimensions ({}x{})",
                       canvas.width(), canvas.height(), settings.width, settings.height);
            }

            let _ = state.canvas.insert(canvas);
        }

        gst::debug!(CAT, imp: self, "Opened shared memory canvas {:?}", settings.flink);
        gst::info!(CAT, imp: self, "Started");

        Ok(())
    }

    fn stop(&self) -> Result<(), gst::ErrorMessage> {
        let mut state = self.state.lock().unwrap();
        let _ = state.canvas.take();

        gst::info!(CAT, imp: self, "Stopped");

        Ok(())
    }

    fn fill(
        &self,
        offset: u64,
        length: u32,
        buffer: &mut gst::BufferRef,
    ) -> Result<gst::FlowSuccess, gst::FlowError> {
        let mut state = self.state.lock().unwrap();
        let canvas = if let Some(canvas) = &mut state.canvas {
            canvas
        } else {
            gst::error!(CAT, imp: self, "shared memory canvas not mapped");
            return Err(gst::FlowError::Error);
        };

        let pixels = canvas.pixel_byte_slice();
        unsafe {
            let mut map = buffer.map_writable().map_err(|_| {
                gst::element_imp_error!(self, gst::LibraryError::Failed, ["Failed to map buffer"]);
                gst::FlowError::Error
            })?;

            std::ptr::copy_nonoverlapping(
                pixels.as_ptr(),
                map.as_mut_ptr(),
                min(length as usize, pixels.len()),
            )
        }
        buffer.set_size(min(length as usize, pixels.len()));

        Ok(gst::FlowSuccess::Ok)
    }
}
