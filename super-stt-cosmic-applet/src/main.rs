// SPDX-License-Identifier: GPL-3.0-only
use clap::{Arg, Command, ValueEnum};
use super_stt_cosmic_applet::{VERSION, VisualizationSide};

#[derive(ValueEnum, Clone, Debug)]
enum Side {
    Full,
    Left,
    Right,
}

impl From<Side> for VisualizationSide {
    fn from(side: Side) -> Self {
        match side {
            Side::Full => VisualizationSide::Full,
            Side::Left => VisualizationSide::Left,
            Side::Right => VisualizationSide::Right,
        }
    }
}

fn main() -> cosmic::iced::Result {
    env_logger::init();
    log::info!("Starting Super STT applet with version {VERSION}");

    let matches = Command::new("super-stt-cosmic-applet")
        .version(VERSION)
        .about("COSMIC panel applet for Super STT speech-to-text service")
        .arg(
            Arg::new("side")
                .long("side")
                .short('s')
                .help("Visualization side to display")
                .value_parser(clap::value_parser!(Side))
                .default_value("full"),
        )
        .get_matches();

    let side = matches.get_one::<Side>("side").unwrap().clone();
    let visualization_side = VisualizationSide::from(side);

    cosmic::applet::run::<super_stt_cosmic_applet::SuperSttApplet>(visualization_side)
}
