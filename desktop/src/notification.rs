//! One-shot UserNotifications entry point for the bundled background gateway.

use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context as _, ensure};
use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_foundation::{NSBundle, NSError, NSString, NSUUID};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNNotificationSound, UNUserNotificationCenter,
};

pub(super) fn run_if_requested() -> Option<anyhow::Result<()>> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != "--deliver-model-notification" {
        return None;
    }
    let [Some(title), Some(subtitle), Some(body), None] =
        [args.next(), args.next(), args.next(), args.next()]
    else {
        return Some(Err(anyhow::anyhow!(
            "expected a notification title, subtitle and body"
        )));
    };
    Some(objc2::rc::autoreleasepool(|_| {
        ensure!(
            NSBundle::mainBundle().bundleIdentifier().is_some(),
            "notifications require the Codex Mixin app bundle"
        );
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let (send, receive) = mpsc::sync_channel(1);
        let authorization = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            let _ = send.try_send((granted.as_bool(), error.is_null()));
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &authorization,
        );
        let (granted, no_error) = receive
            .recv_timeout(Duration::from_secs(10))
            .context("request notification permission")?;
        ensure!(no_error, "could not request notification permission");
        ensure!(granted, "notifications are disabled for Codex Mixin");

        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&title.to_string_lossy()));
        content.setSubtitle(&NSString::from_str(&subtitle.to_string_lossy()));
        content.setBody(&NSString::from_str(&body.to_string_lossy()));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        content.setThreadIdentifier(objc2_foundation::ns_string!("provider-model-changes"));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSUUID::UUID().UUIDString(),
            &content,
            None,
        );
        let (send, receive) = mpsc::sync_channel(1);
        let completed = RcBlock::new(move |error: *mut NSError| {
            let _ = send.try_send(error.is_null());
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&completed));
        ensure!(
            receive
                .recv_timeout(Duration::from_secs(5))
                .context("deliver notification")?,
            "Notification Center rejected the message"
        );
        Ok(())
    }))
}
