use gst::glib;

mod wbsrc;

fn plugin_init(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    wbsrc::register(plugin)?;
    Ok(())
}

gst::plugin_define!(
    wbsrc,
    env!("CARGO_PKG_DESCRIPTION"),
    plugin_init,
    concat!(env!("CARGO_PKG_VERSION"), "-", env!("COMMIT_ID")),
    "GPL-3",
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_REPOSITORY"),
    env!("BUILD_REL_DATE")
);
