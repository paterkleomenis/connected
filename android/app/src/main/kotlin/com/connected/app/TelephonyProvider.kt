package com.connected.app

import android.Manifest
import android.content.BroadcastReceiver
import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.database.Cursor
import android.net.Uri
import android.os.Build
import android.provider.CallLog
import android.provider.ContactsContract
import android.provider.Telephony
import android.telecom.TelecomManager
import android.telephony.SmsManager
import android.telephony.TelephonyManager
import android.util.Log
import androidx.core.content.ContextCompat
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.connected_ffi.*

class TelephonyProvider(private val context: Context) {

    interface TelephonyListener {
        fun onNewSmsReceived(message: FfiSmsMessage)
        fun onCallStateChanged(call: FfiActiveCall?)
    }

    private var listener: TelephonyListener? = null
    private var smsReceiver: BroadcastReceiver? = null
    private var callStateReceiver: BroadcastReceiver? = null

    fun setListener(listener: TelephonyListener?) {
        this.listener = listener
    }

    // ========================================================================
    // Permission Checking
    // ========================================================================

    fun hasContactsPermission(): Boolean {
        return ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.READ_CONTACTS
        ) == PackageManager.PERMISSION_GRANTED
    }

    fun hasSmsPermission(): Boolean {
        return ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.READ_SMS
        ) == PackageManager.PERMISSION_GRANTED &&
                ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.SEND_SMS
                ) == PackageManager.PERMISSION_GRANTED
    }

    fun hasCallLogPermission(): Boolean {
        return ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.READ_CALL_LOG
        ) == PackageManager.PERMISSION_GRANTED
    }

    fun hasPhonePermission(): Boolean {
        return ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.CALL_PHONE
        ) == PackageManager.PERMISSION_GRANTED &&
                ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.READ_PHONE_STATE
                ) == PackageManager.PERMISSION_GRANTED
    }

    fun hasAnswerPhoneCallsPermission(): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.ANSWER_PHONE_CALLS
            ) == PackageManager.PERMISSION_GRANTED
        } else {
            // Permission not required on older versions
            true
        }
    }

    fun getRequiredPermissions(): Array<String> {
        return arrayOf(
            Manifest.permission.READ_CONTACTS,
            Manifest.permission.READ_SMS,
            Manifest.permission.SEND_SMS,
            Manifest.permission.RECEIVE_SMS,
            Manifest.permission.READ_CALL_LOG,
            Manifest.permission.CALL_PHONE,
            Manifest.permission.READ_PHONE_STATE,
            Manifest.permission.ANSWER_PHONE_CALLS
        )
    }

    // ========================================================================
    // Contacts
    // ========================================================================

    suspend fun getContacts(): List<FfiContact> = withContext(Dispatchers.IO) {
        if (!hasContactsPermission()) {
            return@withContext emptyList()
        }

        val contacts = mutableListOf<FfiContact>()
        val resolver = context.contentResolver

        val cursor = resolver.query(
            ContactsContract.Contacts.CONTENT_URI,
            arrayOf(
                ContactsContract.Contacts._ID,
                ContactsContract.Contacts.DISPLAY_NAME_PRIMARY,
                ContactsContract.Contacts.STARRED,
                ContactsContract.Contacts.HAS_PHONE_NUMBER
            ),
            null,
            null,
            ContactsContract.Contacts.DISPLAY_NAME_PRIMARY + " ASC"
        )

        cursor?.use {
            val idIndex = it.getColumnIndex(ContactsContract.Contacts._ID)
            val nameIndex = it.getColumnIndex(ContactsContract.Contacts.DISPLAY_NAME_PRIMARY)
            val starredIndex = it.getColumnIndex(ContactsContract.Contacts.STARRED)
            val hasPhoneIndex = it.getColumnIndex(ContactsContract.Contacts.HAS_PHONE_NUMBER)

            while (it.moveToNext()) {
                val id = it.getString(idIndex) ?: continue
                val name = it.getString(nameIndex) ?: "Unknown"
                val starred = it.getInt(starredIndex) == 1
                val hasPhone = it.getInt(hasPhoneIndex) > 0

                val phoneNumbers = if (hasPhone) getPhoneNumbers(resolver, id) else emptyList()
                val emails = getEmails(resolver, id)

                contacts.add(
                    FfiContact(
                        id = id,
                        name = name,
                        phoneNumbers = phoneNumbers,
                        emails = emails,
                        photo = null, // Skip photos for performance
                        starred = starred
                    )
                )
            }
        }

        contacts
    }

    private fun getPhoneNumbers(resolver: ContentResolver, contactId: String): List<FfiPhoneNumber> {
        val numbers = mutableListOf<FfiPhoneNumber>()

        val cursor = resolver.query(
            ContactsContract.CommonDataKinds.Phone.CONTENT_URI,
            arrayOf(
                ContactsContract.CommonDataKinds.Phone.NUMBER,
                ContactsContract.CommonDataKinds.Phone.TYPE
            ),
            "${ContactsContract.CommonDataKinds.Phone.CONTACT_ID} = ?",
            arrayOf(contactId),
            null
        )

        cursor?.use {
            val numberIndex = it.getColumnIndex(ContactsContract.CommonDataKinds.Phone.NUMBER)
            val typeIndex = it.getColumnIndex(ContactsContract.CommonDataKinds.Phone.TYPE)

            while (it.moveToNext()) {
                val number = it.getString(numberIndex) ?: continue
                val type = it.getInt(typeIndex)

                numbers.add(
                    FfiPhoneNumber(
                        number = number,
                        label = mapPhoneType(type)
                    )
                )
            }
        }

        return numbers
    }

    private fun getEmails(resolver: ContentResolver, contactId: String): List<String> {
        val emails = mutableListOf<String>()

        val cursor = resolver.query(
            ContactsContract.CommonDataKinds.Email.CONTENT_URI,
            arrayOf(ContactsContract.CommonDataKinds.Email.ADDRESS),
            "${ContactsContract.CommonDataKinds.Email.CONTACT_ID} = ?",
            arrayOf(contactId),
            null
        )

        cursor?.use {
            val emailIndex = it.getColumnIndex(ContactsContract.CommonDataKinds.Email.ADDRESS)

            while (it.moveToNext()) {
                val email = it.getString(emailIndex)
                if (!email.isNullOrEmpty()) {
                    emails.add(email)
                }
            }
        }

        return emails
    }

    private fun mapPhoneType(type: Int): PhoneNumberType {
        return when (type) {
            ContactsContract.CommonDataKinds.Phone.TYPE_MOBILE -> PhoneNumberType.MOBILE
            ContactsContract.CommonDataKinds.Phone.TYPE_HOME -> PhoneNumberType.HOME
            ContactsContract.CommonDataKinds.Phone.TYPE_WORK -> PhoneNumberType.WORK
            ContactsContract.CommonDataKinds.Phone.TYPE_MAIN -> PhoneNumberType.MAIN
            else -> PhoneNumberType.OTHER
        }
    }

    // ========================================================================
    // SMS / Conversations
    // ========================================================================

    suspend fun getConversations(): List<FfiConversation> = withContext(Dispatchers.IO) {
        if (!hasSmsPermission()) {
            return@withContext emptyList()
        }

        val conversations = mutableListOf<FfiConversation>()
        val resolver = context.contentResolver

        val cursor = resolver.query(
            Telephony.Sms.Conversations.CONTENT_URI,
            arrayOf(
                Telephony.Sms.Conversations.THREAD_ID,
                Telephony.Sms.Conversations.SNIPPET,
                Telephony.Sms.Conversations.MESSAGE_COUNT
            ),
            null,
            null,
            "date DESC"
        )

        cursor?.use {
            val threadIdIndex = it.getColumnIndex(Telephony.Sms.Conversations.THREAD_ID)
            val snippetIndex = it.getColumnIndex(Telephony.Sms.Conversations.SNIPPET)

            while (it.moveToNext()) {
                val threadId = it.getLong(threadIdIndex).toString()
                val snippet = it.getString(snippetIndex)

                // Get thread details
                val threadDetails = getThreadDetails(resolver, threadId)

                conversations.add(
                    FfiConversation(
                        id = threadId,
                        addresses = threadDetails.addresses,
                        contactNames = threadDetails.contactNames,
                        lastMessage = snippet,
                        lastTimestamp = threadDetails.lastTimestamp,
                        unreadCount = threadDetails.unreadCount.toUInt()
                    )
                )
            }
        }

        conversations
    }

    private data class ThreadDetails(
        val addresses: List<String>,
        val contactNames: List<String>,
        val lastTimestamp: ULong,
        val unreadCount: Int
    )

    private fun getThreadDetails(resolver: ContentResolver, threadId: String): ThreadDetails {
        val addresses = mutableSetOf<String>()
        var lastTimestamp = 0L
        var unreadCount = 0

        val cursor = resolver.query(
            Telephony.Sms.CONTENT_URI,
            arrayOf(
                Telephony.Sms.ADDRESS,
                Telephony.Sms.DATE,
                Telephony.Sms.READ
            ),
            "${Telephony.Sms.THREAD_ID} = ?",
            arrayOf(threadId),
            "${Telephony.Sms.DATE} DESC"
        )

        cursor?.use {
            val addressIndex = it.getColumnIndex(Telephony.Sms.ADDRESS)
            val dateIndex = it.getColumnIndex(Telephony.Sms.DATE)
            val readIndex = it.getColumnIndex(Telephony.Sms.READ)

            while (it.moveToNext()) {
                val address = it.getString(addressIndex)
                if (!address.isNullOrEmpty()) {
                    addresses.add(address)
                }

                val date = it.getLong(dateIndex)
                if (date > lastTimestamp) {
                    lastTimestamp = date
                }

                val read = it.getInt(readIndex)
                if (read == 0) {
                    unreadCount++
                }
            }
        }

        val contactNames = addresses.map { getContactNameForNumber(it) ?: it }

        return ThreadDetails(
            addresses = addresses.toList(),
            contactNames = contactNames,
            lastTimestamp = lastTimestamp.toULong(),
            unreadCount = unreadCount
        )
    }

    suspend fun getMessages(threadId: String, limit: Int = 50): List<FfiSmsMessage> =
        withContext(Dispatchers.IO) {
            if (!hasSmsPermission()) {
                return@withContext emptyList()
            }

            val messages = mutableListOf<FfiSmsMessage>()
            val resolver = context.contentResolver

            // Build URI with limit parameter (works on all Android versions)
            val uri = Telephony.Sms.CONTENT_URI.buildUpon()
                .appendQueryParameter("limit", limit.toString())
                .build()

            val cursor = resolver.query(
                uri,
                arrayOf(
                    Telephony.Sms._ID,
                    Telephony.Sms.THREAD_ID,
                    Telephony.Sms.ADDRESS,
                    Telephony.Sms.BODY,
                    Telephony.Sms.DATE,
                    Telephony.Sms.TYPE,
                    Telephony.Sms.READ,
                    Telephony.Sms.STATUS
                ),
                "${Telephony.Sms.THREAD_ID} = ?",
                arrayOf(threadId),
                "${Telephony.Sms.DATE} DESC"
            )

            cursor?.use {
                val idIndex = it.getColumnIndex(Telephony.Sms._ID)
                val threadIndex = it.getColumnIndex(Telephony.Sms.THREAD_ID)
                val addressIndex = it.getColumnIndex(Telephony.Sms.ADDRESS)
                val bodyIndex = it.getColumnIndex(Telephony.Sms.BODY)
                val dateIndex = it.getColumnIndex(Telephony.Sms.DATE)
                val typeIndex = it.getColumnIndex(Telephony.Sms.TYPE)
                val readIndex = it.getColumnIndex(Telephony.Sms.READ)
                val statusIndex = it.getColumnIndex(Telephony.Sms.STATUS)

                while (it.moveToNext()) {
                    val id = it.getString(idIndex) ?: continue
                    val thread = it.getString(threadIndex) ?: threadId
                    val address = it.getString(addressIndex) ?: ""
                    val body = it.getString(bodyIndex) ?: ""
                    val date = it.getLong(dateIndex)
                    val type = it.getInt(typeIndex)
                    val read = it.getInt(readIndex) == 1
                    val status = it.getInt(statusIndex)

                    val isOutgoing = type == Telephony.Sms.MESSAGE_TYPE_SENT ||
                            type == Telephony.Sms.MESSAGE_TYPE_OUTBOX

                    messages.add(
                        FfiSmsMessage(
                            id = id,
                            threadId = thread,
                            address = address,
                            contactName = getContactNameForNumber(address),
                            body = body,
                            timestamp = date.toULong(),
                            isOutgoing = isOutgoing,
                            isRead = read,
                            status = mapSmsStatus(type, status)
                        )
                    )
                }
            }

            messages.reversed() // Return in chronological order
        }

    private fun mapSmsStatus(type: Int, status: Int): SmsStatus {
        return when {
            type == Telephony.Sms.MESSAGE_TYPE_INBOX -> SmsStatus.RECEIVED
            type == Telephony.Sms.MESSAGE_TYPE_OUTBOX -> SmsStatus.PENDING
            type == Telephony.Sms.MESSAGE_TYPE_FAILED -> SmsStatus.FAILED
            status == Telephony.Sms.STATUS_COMPLETE -> SmsStatus.DELIVERED
            type == Telephony.Sms.MESSAGE_TYPE_SENT -> SmsStatus.SENT
            else -> SmsStatus.PENDING
        }
    }

    fun sendSms(to: String, body: String): Result<String> {
        return try {
            if (!hasSmsPermission()) {
                return Result.failure(SecurityException("SMS permission not granted"))
            }

            val smsManager = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                context.getSystemService(SmsManager::class.java)
            } else {
                @Suppress("DEPRECATION")
                SmsManager.getDefault()
            }

            // Split long messages if needed
            val parts = smsManager.divideMessage(body)
            if (parts.size > 1) {
                smsManager.sendMultipartTextMessage(to, null, parts, null, null)
            } else {
                smsManager.sendTextMessage(to, null, body, null, null)
            }

            Result.success("sent")
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    // ========================================================================
    // Call Log
    // ========================================================================

    suspend fun getCallLog(limit: Int = 50): List<FfiCallLogEntry> = withContext(Dispatchers.IO) {
        if (!hasCallLogPermission()) {
            return@withContext emptyList()
        }

        val entries = mutableListOf<FfiCallLogEntry>()
        val resolver = context.contentResolver

        // Build URI with limit parameter (works on all Android versions)
        val uri = CallLog.Calls.CONTENT_URI.buildUpon()
            .appendQueryParameter("limit", limit.toString())
            .build()

        val cursor = resolver.query(
            uri,
            arrayOf(
                CallLog.Calls._ID,
                CallLog.Calls.NUMBER,
                CallLog.Calls.CACHED_NAME,
                CallLog.Calls.TYPE,
                CallLog.Calls.DATE,
                CallLog.Calls.DURATION,
                CallLog.Calls.IS_READ
            ),
            null,
            null,
            "${CallLog.Calls.DATE} DESC"
        )

        cursor?.use {
            val idIndex = it.getColumnIndex(CallLog.Calls._ID)
            val numberIndex = it.getColumnIndex(CallLog.Calls.NUMBER)
            val nameIndex = it.getColumnIndex(CallLog.Calls.CACHED_NAME)
            val typeIndex = it.getColumnIndex(CallLog.Calls.TYPE)
            val dateIndex = it.getColumnIndex(CallLog.Calls.DATE)
            val durationIndex = it.getColumnIndex(CallLog.Calls.DURATION)
            val readIndex = it.getColumnIndex(CallLog.Calls.IS_READ)

            while (it.moveToNext()) {
                val id = it.getString(idIndex) ?: continue
                val number = it.getString(numberIndex) ?: ""
                val name = it.getString(nameIndex)
                val type = it.getInt(typeIndex)
                val date = it.getLong(dateIndex)
                val duration = it.getInt(durationIndex)
                val read = it.getInt(readIndex) == 1

                entries.add(
                    FfiCallLogEntry(
                        id = id,
                        number = number,
                        contactName = name,
                        callType = mapCallType(type),
                        timestamp = date.toULong(),
                        duration = duration.toUInt(),
                        isRead = read
                    )
                )
            }
        }

        entries
    }

    private fun mapCallType(type: Int): CallType {
        return when (type) {
            CallLog.Calls.INCOMING_TYPE -> CallType.INCOMING
            CallLog.Calls.OUTGOING_TYPE -> CallType.OUTGOING
            CallLog.Calls.MISSED_TYPE -> CallType.MISSED
            CallLog.Calls.REJECTED_TYPE -> CallType.REJECTED
            CallLog.Calls.BLOCKED_TYPE -> CallType.BLOCKED
            CallLog.Calls.VOICEMAIL_TYPE -> CallType.VOICEMAIL
            else -> CallType.INCOMING
        }
    }

    // ========================================================================
    // Phone Calls
    // ========================================================================

    fun initiateCall(number: String): Boolean {
        return try {
            if (!hasPhonePermission()) {
                return false
            }

            val intent = Intent(Intent.ACTION_CALL).apply {
                data = Uri.parse("tel:$number")
                flags = Intent.FLAG_ACTIVITY_NEW_TASK
            }
            context.startActivity(intent)
            true
        } catch (e: Exception) {
            false
        }
    }

    fun performCallAction(action: CallAction): Boolean {
        return try {
            // Check for ANSWER_PHONE_CALLS permission before performing actions
            if (!hasAnswerPhoneCallsPermission()) {
                Log.w("TelephonyProvider", "ANSWER_PHONE_CALLS permission not granted")
                return false
            }

            val telecomManager = context.getSystemService(Context.TELECOM_SERVICE) as TelecomManager

            when (action) {
                CallAction.Answer -> {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                        telecomManager.acceptRingingCall()
                        Log.d("TelephonyProvider", "Call answered via TelecomManager")
                        true
                    } else {
                        Log.w("TelephonyProvider", "Answer call not supported on this Android version")
                        false
                    }
                }

                CallAction.Reject, CallAction.HangUp -> {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                        val result = telecomManager.endCall()
                        Log.d("TelephonyProvider", "Call ended via TelecomManager: $result")
                        result
                    } else {
                        // For older versions, we'd need to use reflection or ITelephony
                        Log.w("TelephonyProvider", "End call not supported on this Android version")
                        false
                    }
                }

                CallAction.Mute, CallAction.Unmute, CallAction.Hold, CallAction.Unhold, is CallAction.SendDtmf -> {
                    // These actions require additional implementation via InCallService
                    Log.w("TelephonyProvider", "Call action $action not yet implemented")
                    false
                }
            }
        } catch (e: SecurityException) {
            Log.e("TelephonyProvider", "Security exception performing call action: ${e.message}")
            false
        } catch (e: Exception) {
            Log.e("TelephonyProvider", "Exception performing call action: ${e.message}")
            false
        }
    }

    // ========================================================================
    // Helpers
    // ========================================================================

    private fun getContactNameForNumber(phoneNumber: String): String? {
        if (!hasContactsPermission() || phoneNumber.isEmpty()) {
            return null
        }

        val uri = Uri.withAppendedPath(
            ContactsContract.PhoneLookup.CONTENT_FILTER_URI,
            Uri.encode(phoneNumber)
        )

        val cursor = context.contentResolver.query(
            uri,
            arrayOf(ContactsContract.PhoneLookup.DISPLAY_NAME),
            null,
            null,
            null
        )

        return cursor?.use {
            if (it.moveToFirst()) {
                it.getString(it.getColumnIndex(ContactsContract.PhoneLookup.DISPLAY_NAME))
            } else {
                null
            }
        }
    }

    // ========================================================================
    // Broadcast Receivers
    // ========================================================================

    fun registerReceivers() {
        // SMS Receiver
        if (hasSmsPermission()) {
            smsReceiver = object : BroadcastReceiver() {
                override fun onReceive(context: Context?, intent: Intent?) {
                    if (intent?.action == Telephony.Sms.Intents.SMS_RECEIVED_ACTION) {
                        val messages = Telephony.Sms.Intents.getMessagesFromIntent(intent)
                        messages?.forEach { smsMessage ->
                            val ffiMessage = FfiSmsMessage(
                                id = System.currentTimeMillis().toString(),
                                threadId = "",
                                address = smsMessage.originatingAddress ?: "",
                                contactName = getContactNameForNumber(
                                    smsMessage.originatingAddress ?: ""
                                ),
                                body = smsMessage.messageBody ?: "",
                                timestamp = smsMessage.timestampMillis.toULong(),
                                isOutgoing = false,
                                isRead = false,
                                status = SmsStatus.RECEIVED
                            )
                            listener?.onNewSmsReceived(ffiMessage)
                        }
                    }
                }
            }

            val smsFilter = IntentFilter(Telephony.Sms.Intents.SMS_RECEIVED_ACTION)
            context.registerReceiver(smsReceiver, smsFilter)
        }

        // Call State Receiver
        if (hasPhonePermission()) {
            callStateReceiver = object : BroadcastReceiver() {
                override fun onReceive(context: Context?, intent: Intent?) {
                    if (intent?.action == TelephonyManager.ACTION_PHONE_STATE_CHANGED) {
                        val state = intent.getStringExtra(TelephonyManager.EXTRA_STATE)
                        val number =
                            intent.getStringExtra(TelephonyManager.EXTRA_INCOMING_NUMBER) ?: ""

                        val activeCall = when (state) {
                            TelephonyManager.EXTRA_STATE_RINGING -> FfiActiveCall(
                                number = number,
                                contactName = getContactNameForNumber(number),
                                state = ActiveCallState.RINGING,
                                duration = 0u,
                                isIncoming = true
                            )

                            TelephonyManager.EXTRA_STATE_OFFHOOK -> FfiActiveCall(
                                number = number,
                                contactName = getContactNameForNumber(number),
                                state = ActiveCallState.CONNECTED,
                                duration = 0u,
                                isIncoming = true
                            )

                            TelephonyManager.EXTRA_STATE_IDLE -> null
                            else -> null
                        }

                        listener?.onCallStateChanged(activeCall)
                    }
                }
            }

            val callFilter = IntentFilter(TelephonyManager.ACTION_PHONE_STATE_CHANGED)
            context.registerReceiver(callStateReceiver, callFilter)
        }
    }

    fun unregisterReceivers() {
        smsReceiver?.let {
            try {
                context.unregisterReceiver(it)
            } catch (e: Exception) {
                // Ignore if not registered
            }
            smsReceiver = null
        }

        callStateReceiver?.let {
            try {
                context.unregisterReceiver(it)
            } catch (e: Exception) {
                // Ignore if not registered
            }
            callStateReceiver = null
        }
    }
}
