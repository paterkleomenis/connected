# Play Store Permission Justification Document

**App Name:** Connected
**Package:** com.connected.app.sync
**Version:** 2.9.4
**Date:** April 6, 2026

---

## Core App Functionality

Connected is a cross-platform device synchronization app that enables users to:
1. Transfer files between their devices on the same local network
2. Synchronize clipboard content between devices
3. Link phones to desktop computers for SMS, calls, and media sync
4. Monitor and sync media playback between devices

All communication happens directly between user-owned devices on the local network. **No data is sent to remote servers.**

---

## Sensitive Permission Justifications

### 1. SMS Permissions (`READ_SMS`, `SEND_SMS`, `RECEIVE_SMS`)

**Core Feature:** Phone Link - SMS Sync

**Why needed:**
- `READ_SMS`: Read existing SMS messages to display and sync them to the user's linked desktop device
- `SEND_SMS`: Allow users to send SMS messages from their linked desktop device
- `RECEIVE_SMS`: Receive incoming SMS notifications to forward to linked devices in real-time

**How users benefit:** Users can read and respond to their phone's text messages from their computer, which is the primary use case of the Phone Link feature.

**Alternative considered:** Android does not provide alternative APIs for SMS access. These permissions are the only way to implement SMS sync functionality.

---

### 2. Contacts Permission (`READ_CONTACTS`)

**Core Feature:** Phone Link - Contact Display

**Why needed:**
- Display contact names (instead of raw phone numbers) when syncing calls and SMS messages to linked devices
- Provide a familiar contact picker interface when selecting recipients for messages

**How users benefit:** Users see recognizable names instead of phone numbers when viewing synced calls and messages on their desktop.

**Alternative considered:** We could display only raw phone numbers, but this would significantly degrade user experience. No alternative APIs provide contact lookup functionality.

---

### 3. Call Log Permission (`READ_CALL_LOG`)

**Core Feature:** Phone Link - Call History Sync

**Why needed:**
- Sync call history (incoming, outgoing, missed calls) to linked desktop devices
- Display call details (contact, duration, timestamp) on the desktop companion app

**How users benefit:** Users can view their phone's call history from their computer and identify missed calls.

**Alternative considered:** Android does not provide alternative APIs for call log access.

---

### 4. Phone Permission (`CALL_PHONE`)

**Core Feature:** Phone Link - Initiate Calls from Desktop

**Why needed:**
- Allow users to initiate phone calls from their linked desktop device
- Provide dial-back functionality from the desktop interface

**How users benefit:** Convenience of placing calls from the computer interface while using the phone's cellular connection.

**Alternative considered:** We could use an intent to open the dialer with a pre-filled number (`ACTION_DIAL`), but this requires manual user confirmation. `CALL_PHONE` enables a seamless experience matching native Phone Link apps.

---

### 5. External Storage (`MANAGE_EXTERNAL_STORAGE`)

**Core Feature:** File Transfer

**Why needed:**
- Access all files on the device for transfer to linked devices
- Save received files to appropriate directories (Downloads, Documents, etc.)
- Handle arbitrary file types without restrictions

**How users benefit:** Users can transfer any file type between their devices without limitations, matching the functionality of desktop file managers.

**Alternative considered:** The Storage Access Framework (SAF) is too limited for our use case - users cannot browse and select arbitrary folders for bulk transfers, and it doesn't support receiving files in the background.

---

### 6. Battery Optimization (`REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`)

**Core Feature:** Persistent Device Connection

**Why needed:**
- Maintain persistent connection to linked devices while the app is in the background
- Prevent Android from killing the foreground service that handles device communication
- Ensure file transfers and SMS/call sync complete without interruption

**How users benefit:** Reliable, uninterrupted synchronization and file transfers even when the app is not actively in use.

**Alternative considered:** We already use a foreground service with proper notification. However, battery optimization can still interrupt long-running transfers. This permission is a safeguard for core functionality.

---

### 7. Location Permissions (`ACCESS_COARSE_LOCATION`, `ACCESS_FINE_LOCATION`)

**Core Feature:** Bluetooth Device Discovery

**Why needed:**
- Android 12+ requires location permissions for Bluetooth LE scanning
- Used ONLY for discovering nearby devices for pairing
- We do not collect, store, or transmit location data

**How users benefit:** Enables automatic discovery of nearby devices for pairing without manual IP entry.

**Alternative considered:** Manual IP entry is available as a fallback, but automatic discovery significantly improves user experience. Android does not provide a way to use Bluetooth LE scanning without location permissions.

---

### 8. Notification Listener (`BIND_NOTIFICATION_LISTENER_SERVICE`)

**Core Feature:** Media Playback Sync

**Why needed:**
- Monitor media playback notifications (music, podcasts, videos)
- Sync playback state and controls to linked devices
- Enable remote media control from desktop companion app

**How users benefit:** Users can see what's playing on their phone and control playback from their computer.

**Alternative considered:** Android does not provide alternative APIs for reading other apps' notifications. This is the only way to implement media sync.

---

## Data Handling Summary

| Permission | Data Collected | Data Stored | Data Transmitted |
|------------|---------------|-------------|------------------|
| SMS | SMS content & metadata | No (in-memory only during sync) | Only to user's linked devices |
| Contacts | Contact names & numbers | No (looked up on-demand) | Only to user's linked devices |
| Call Log | Call history | No (in-memory only during sync) | Only to user's linked devices |
| Phone | Phone numbers for dialing | No | N/A (initiates calls) |
| Storage | User-selected files | No (transferred directly) | Only to user's linked devices |
| Location | None | None | None |
| Notifications | Media playback info | No (in-memory only) | Only to user's linked devices |

---

## Security Measures

1. **Local-Only Communication**: All data transfers occur directly between devices on the local network using QUIC encryption
2. **No Cloud Servers**: We do not operate any cloud servers that process or store user data
3. **User Consent**: All permissions are requested at runtime with clear explanations
4. **Minimal Data Retention**: Data is processed in-memory and not persisted beyond the active sync session
5. **Open Source**: The app's source code is available for audit at [GitHub repository]

---

## User-Facing Explanations

When requesting permissions, the app displays a pre-permission rationale explaining:
- **Why** the permission is needed
- **What** functionality it enables
- **How** the data will be used (locally, between user's devices only)

Users can deny any permission and still use core file transfer features.
