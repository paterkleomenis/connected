# Google Play review instructions: Phone Link

Use this checklist when completing the SMS/Call Log Permissions Declaration and
recording the reviewer video.

## Declared use case

Select:

> Cross-device synchronization or transfer of SMS or calls

The Android Play build uses these permissions for that use case:

- `READ_SMS`: read existing conversations for synchronization
- `SEND_SMS`: send a message requested from the linked desktop
- `RECEIVE_SMS`: forward newly received SMS messages to the linked desktop
- `READ_CALL_LOG`: display synchronized call history on the linked desktop

`READ_CONTACTS` is used only to resolve names for messages and call-history
entries. The app does not request call-control permissions. An outgoing call
request only opens the Android system dialer with the number prefilled; the user
must confirm the call on the phone. The app cannot answer, reject, or hang up
calls remotely.

## Reviewer access instructions

1. Install Connected on an Android phone and install the Connected desktop app
   on a computer on the same local network.
2. Open both apps and pair the devices. Accept the pairing request and mark the
   desktop device as trusted.
3. On Android, open **Settings → Phone Link**, choose **Grant Permissions**,
   accept the Contacts, SMS, and Call Log permissions, then enable **Phone
   Link**.
4. On the trusted desktop device, open its options menu and choose **Request
   Conversations**.
5. Open a conversation and verify that the existing messages shown on the
   Android phone appear on the desktop.
6. Send a test SMS from the desktop. Show the message arriving on the phone.
7. Send a test SMS to the phone from a second phone or test account. Show the
   incoming message being forwarded to the desktop.
8. From the same device options menu, choose **Request Call Log** and show the
   phone's call-history entries on the desktop.
9. If demonstrating outgoing calls, start a call request from the desktop and
   show that Android opens the system dialer for user confirmation.

## Video requirements

Record the complete flow above with both screens visible when possible. The
video must show real data moving between the Android phone and the linked
desktop; a settings screen, permission grant, or static feature description by
itself is not sufficient. Use test messages and test call history, not private
personal content.

The first seconds of the video should identify the Android device, the desktop
device, and the pairing relationship. Keep the recording continuous so the
reviewer can follow:

`permission grant → Phone Link enabled → request from desktop → data displayed → new SMS transferred`

For policy details, see Google's [SMS and Call Log permissions policy](https://support.google.com/googleplay/android-developer/answer/10208820)
and [Permissions and APIs that Access Sensitive Information](https://support.google.com/googleplay/android-developer/answer/16558241).
