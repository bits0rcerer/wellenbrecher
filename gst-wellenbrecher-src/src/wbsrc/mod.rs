use gst::glib;
use gst::prelude::*;

mod imp;

glib::wrapper! {
    pub struct WellenbrecherSrc(ObjectSubclass<imp::WellenbrecherSrc>) @extends gst_base::BaseSrc, gst::Element, gst::Object;
}

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(
        Some(plugin),
        "wbsrc",
        gst::Rank::NONE,
        WellenbrecherSrc::static_type(),
    )
}
