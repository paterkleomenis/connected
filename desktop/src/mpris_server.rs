use connected_core::MediaCommand;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
#[cfg(target_os = "linux")]
use tracing::warn;

#[cfg(target_os = "linux")]
use ::mpris_server::{Metadata, PlaybackStatus, Player};
#[cfg(target_os = "linux")]
use async_std::channel::{self, Receiver, Sender};
#[cfg(target_os = "linux")]
use futures_util::future::FutureExt;
#[cfg(target_os = "linux")]
use futures_util::select;

#[derive(Clone, Debug, PartialEq)]
pub struct MprisUpdate {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playing: bool,
}

#[cfg(target_os = "linux")]
static MPRIS_SENDER: OnceLock<Sender<MprisUpdate>> = OnceLock::new();

#[cfg(target_os = "linux")]
pub fn init_mpris(command_tx: std::sync::mpsc::Sender<MediaCommand>) -> bool {
    let (tx, rx) = channel::unbounded();
    if MPRIS_SENDER.set(tx).is_err() {
        return false;
    }

    std::thread::spawn(move || {
        async_std::task::block_on(async move {
            if let Err(e) = run_mpris_server(rx, command_tx).await {
                warn!("MPRIS server failed: {}", e);
            }
        });
    });

    true
}

#[cfg(not(target_os = "linux"))]
pub fn init_mpris(_command_tx: std::sync::mpsc::Sender<MediaCommand>) -> bool {
    false
}

#[cfg(target_os = "linux")]
pub fn send_mpris_update(update: MprisUpdate) {
    if let Some(tx) = MPRIS_SENDER.get() {
        let _ = tx.try_send(update);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn send_mpris_update(_update: MprisUpdate) {}

#[cfg(target_os = "linux")]
async fn run_mpris_server(
    rx: Receiver<MprisUpdate>,
    command_tx: std::sync::mpsc::Sender<MediaCommand>,
) -> ::mpris_server::zbus::Result<()> {
    let player = Player::builder("connected")
        .identity("Connected")
        .desktop_entry("connected")
        .can_play(true)
        .can_pause(true)
        .can_go_next(true)
        .can_go_previous(true)
        .can_control(true)
        .build()
        .await?;

    let sender = command_tx.clone();
    player.connect_play(move |_| {
        let _ = sender.send(MediaCommand::Play);
    });

    let sender = command_tx.clone();
    player.connect_pause(move |_| {
        let _ = sender.send(MediaCommand::Pause);
    });

    let sender = command_tx.clone();
    player.connect_play_pause(move |_| {
        let _ = sender.send(MediaCommand::PlayPause);
    });

    let sender = command_tx.clone();
    player.connect_next(move |_| {
        let _ = sender.send(MediaCommand::Next);
    });

    let sender = command_tx.clone();
    player.connect_previous(move |_| {
        let _ = sender.send(MediaCommand::Previous);
    });

    player.connect_stop(move |_| {
        let _ = command_tx.send(MediaCommand::Stop);
    });

    let mut run_task = player.run().fuse();
    let mut last_update: Option<MprisUpdate> = None;

    loop {
        select! {
            _ = run_task => break,
            update = rx.recv().fuse() => {
                let update = match update {
                    Ok(update) => update,
                    Err(_) => break,
                };
                if last_update.as_ref() == Some(&update) {
                    continue;
                }
                if let Err(e) = apply_update(&player, &update).await {
                    warn!("MPRIS update failed: {}", e);
                }
                last_update = Some(update);
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
async fn apply_update(player: &Player, update: &MprisUpdate) -> ::mpris_server::zbus::Result<()> {
    let has_metadata = update.title.is_some() || update.artist.is_some() || update.album.is_some();

    let status = if update.playing {
        PlaybackStatus::Playing
    } else if has_metadata {
        PlaybackStatus::Paused
    } else {
        PlaybackStatus::Stopped
    };

    let mut builder = Metadata::builder();
    if let Some(title) = &update.title {
        builder = builder.title(title.clone());
    }
    if let Some(artist) = &update.artist {
        builder = builder.artist([artist.clone()]);
    }
    if let Some(album) = &update.album {
        builder = builder.album(album.clone());
    }
    let metadata = if has_metadata {
        builder.build()
    } else {
        Metadata::new()
    };

    player.set_playback_status(status).await?;
    player.set_metadata(metadata).await?;
    Ok(())
}
