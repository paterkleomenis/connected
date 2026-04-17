package com.connected.app.sync

import android.Manifest
import android.content.BroadcastReceiver
import android.database.ContentObserver
import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
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
import androidx.core.net.toUri
import java.io.ByteArrayOutputStream
import java.util.Base64

class TelephonyProvider(private val context: Context) {

    companion object {
        private val MMS_SMS_CONVERSATIONS_URI: Uri = Uri.parse("content://mms-sms/conversations?simple=true")
        private val MMS_PARTS_URI: Uri = Uri.parse("content://mms/part")

        private const val MMS_ADDRESS_TYPE_FROM = 137
        private const val MMS_ADDRESS_TYPE_TO = 151

        private const val MMS_MESSAGE_BOX_INBOX = 1
        private const val MMS_MESSAGE_BOX_SENT = 2
        private const val MMS_MESSAGE_BOX_DRAFT = 3
        private const val MMS_MESSAGE_BOX_OUTBOX = 4
        private const val MMS_MESSAGE_BOX_FAILED = 5

        private const val MMS_INLINE_PAYLOAD_BUDGET_BYTES = 105 * 1024 * 1024
    }

    interface TelephonyListener {
        fun onNewSmsReceived(message: FfiSmsMessage)
        fun onCallStateChanged(call: FfiActiveCall?)
    }

    private var listener: TelephonyListener? = null
    private var smsReceiver: BroadcastReceiver? = null
    private var callStateReceiver: BroadcastReceiver? = null
    private var mmsObserver: ContentObserver? = null
    private val observerHandler = Handler(Looper.getMainLooper())
    private var pendingMmsDispatch: Runnable? = null
    private var lastNotifiedIncomingMmsId: String? = null

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
                ) == PackageManager.PERMISSION_GRANTED &&
                ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.RECEIVE_SMS
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
        return ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.ANSWER_PHONE_CALLS
        ) == PackageManager.PERMISSION_GRANTED
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
                if (idIndex < 0) continue
                val id = it.getString(idIndex) ?: continue

                val name = if (nameIndex >= 0) it.getString(nameIndex) ?: "Unknown" else "Unknown"
                val starred = if (starredIndex >= 0) it.getInt(starredIndex) == 1 else false
                val hasPhone = if (hasPhoneIndex >= 0) it.getInt(hasPhoneIndex) > 0 else false

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

            if (numberIndex >= 0) {
                while (it.moveToNext()) {
                    val number = it.getString(numberIndex) ?: continue
                    val type =
                        if (typeIndex >= 0) it.getInt(typeIndex) else ContactsContract.CommonDataKinds.Phone.TYPE_OTHER

                    numbers.add(
                        FfiPhoneNumber(
                            number = number,
                            label = mapPhoneType(type)
                        )
                    )
                }
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

            if (emailIndex >= 0) {
                while (it.moveToNext()) {
                    val email = it.getString(emailIndex)
                    if (!email.isNullOrEmpty()) {
                        emails.add(email)
                    }
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

        val cursor = queryConversationsCursor(resolver)

        cursor?.use {
            val threadIdIndex = it.getColumnIndex("_id")
                .takeIf { index -> index >= 0 }
                ?: it.getColumnIndex(Telephony.Sms.Conversations.THREAD_ID)
            val snippetIndex = it.getColumnIndex("snippet")
                .takeIf { index -> index >= 0 }
                ?: it.getColumnIndex(Telephony.Sms.Conversations.SNIPPET)

            if (threadIdIndex >= 0) {
                while (it.moveToNext()) {
                    val threadId = it.getString(threadIdIndex) ?: continue
                    val snippet = if (snippetIndex >= 0) it.getString(snippetIndex) else null

                    // Get thread details
                    val threadDetails = getThreadDetails(resolver, threadId)
                    val lastMessage = snippet?.takeIf { text -> text.isNotBlank() }
                        ?: threadDetails.lastMessage

                    conversations.add(
                        FfiConversation(
                            id = threadId,
                            addresses = threadDetails.addresses,
                            contactNames = threadDetails.contactNames,
                            lastMessage = lastMessage,
                            lastTimestamp = threadDetails.lastTimestamp,
                            unreadCount = threadDetails.unreadCount.toUInt()
                        )
                    )
                }
            }
        }

        conversations.sortedByDescending { convo -> convo.lastTimestamp }
    }

    private data class ThreadDetails(
        val addresses: List<String>,
        val contactNames: List<String>,
        val lastTimestamp: ULong,
        val unreadCount: Int,
        val lastMessage: String?
    )

    private fun queryConversationsCursor(resolver: ContentResolver) =
        try {
            resolver.query(
                MMS_SMS_CONVERSATIONS_URI,
                arrayOf("_id", "snippet"),
                null,
                null,
                "date DESC"
            )
        } catch (_: Exception) {
            null
        } ?: resolver.query(
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

    private fun getThreadDetails(resolver: ContentResolver, threadId: String): ThreadDetails {
        val addresses = linkedSetOf<String>()
        var lastTimestamp = 0L
        var unreadCount = 0
        var lastMessage: String? = null

        val smsCursor = resolver.query(
            Telephony.Sms.CONTENT_URI,
            arrayOf(
                Telephony.Sms.ADDRESS,
                Telephony.Sms.DATE,
                Telephony.Sms.READ,
                Telephony.Sms.BODY
            ),
            "${Telephony.Sms.THREAD_ID} = ?",
            arrayOf(threadId),
            "${Telephony.Sms.DATE} DESC"
        )

        smsCursor?.use {
            val addressIndex = it.getColumnIndex(Telephony.Sms.ADDRESS)
            val dateIndex = it.getColumnIndex(Telephony.Sms.DATE)
            val readIndex = it.getColumnIndex(Telephony.Sms.READ)
            val bodyIndex = it.getColumnIndex(Telephony.Sms.BODY)

            while (it.moveToNext()) {
                val address = if (addressIndex >= 0) it.getString(addressIndex) else null
                if (!address.isNullOrEmpty()) {
                    addresses.add(address)
                }

                if (dateIndex >= 0) {
                    val date = it.getLong(dateIndex)
                    if (date > lastTimestamp) {
                        lastTimestamp = date
                        if (bodyIndex >= 0) {
                            lastMessage = it.getString(bodyIndex)
                        }
                    }
                }

                if (readIndex >= 0) {
                    val read = it.getInt(readIndex)
                    if (read == 0) {
                        unreadCount++
                    }
                }
            }
        }

        val mmsCursor = resolver.query(
            Telephony.Mms.CONTENT_URI,
            arrayOf(
                Telephony.Mms._ID,
                Telephony.Mms.DATE,
                Telephony.Mms.READ,
                Telephony.Mms.MESSAGE_BOX
            ),
            "${Telephony.Mms.THREAD_ID} = ?",
            arrayOf(threadId),
            "${Telephony.Mms.DATE} DESC"
        )

        mmsCursor?.use {
            val idIndex = it.getColumnIndex(Telephony.Mms._ID)
            val dateIndex = it.getColumnIndex(Telephony.Mms.DATE)
            val readIndex = it.getColumnIndex(Telephony.Mms.READ)
            val msgBoxIndex = it.getColumnIndex(Telephony.Mms.MESSAGE_BOX)

            while (it.moveToNext()) {
                val mmsId = if (idIndex >= 0) it.getString(idIndex) else null

                if (dateIndex >= 0) {
                    val rawDate = it.getLong(dateIndex)
                    val date = normalizeMmsTimestamp(rawDate)
                    if (date > lastTimestamp) {
                        lastTimestamp = date
                        if (!mmsId.isNullOrEmpty()) {
                            lastMessage = getMmsPreviewText(resolver, mmsId)
                        }
                    }
                }

                if (readIndex >= 0 && it.getInt(readIndex) == 0) {
                    unreadCount++
                }

                if (!mmsId.isNullOrEmpty()) {
                    val msgBox = if (msgBoxIndex >= 0) it.getInt(msgBoxIndex) else MMS_MESSAGE_BOX_INBOX
                    val address = getMmsAddress(resolver, mmsId, isMmsOutgoing(msgBox))
                    if (address.isNotEmpty()) {
                        addresses.add(address)
                    }
                }
            }
        }

        val contactNames = addresses.map { getContactNameForNumber(it) ?: it }

        return ThreadDetails(
            addresses = addresses.toList(),
            contactNames = contactNames,
            lastTimestamp = lastTimestamp.toULong(),
            unreadCount = unreadCount,
            lastMessage = lastMessage?.takeIf { msg -> msg.isNotBlank() }
        )
    }

    suspend fun getMessages(threadId: String, limit: Int = 50): List<FfiSmsMessage> =
        withContext(Dispatchers.IO) {
            if (!hasSmsPermission()) {
                return@withContext emptyList()
            }

            val resolver = context.contentResolver

            val smsMessages = getSmsMessages(resolver, threadId, limit)
            val mmsMessages = getMmsMessages(resolver, threadId, limit)

            (smsMessages + mmsMessages)
                .sortedByDescending { msg -> msg.timestamp }
                .take(limit)
                .sortedBy { msg -> msg.timestamp }
        }

    private fun getSmsMessages(
        resolver: ContentResolver,
        threadId: String,
        limit: Int
    ): List<FfiSmsMessage> {
        val messages = mutableListOf<FfiSmsMessage>()

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
                if (idIndex < 0) continue
                val id = it.getString(idIndex) ?: continue

                val thread = if (threadIndex >= 0) it.getString(threadIndex) ?: threadId else threadId
                val address = if (addressIndex >= 0) it.getString(addressIndex) ?: "" else ""
                val body = if (bodyIndex >= 0) it.getString(bodyIndex) ?: "" else ""
                val date = if (dateIndex >= 0) it.getLong(dateIndex) else 0L
                val type = if (typeIndex >= 0) it.getInt(typeIndex) else 0
                val read = if (readIndex >= 0) it.getInt(readIndex) == 1 else false
                val status = if (statusIndex >= 0) it.getInt(statusIndex) else -1

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
                        status = mapSmsStatus(type, status),
                        attachments = emptyList()
                    )
                )
            }
        }

        return messages
    }

    private fun getMmsMessages(
        resolver: ContentResolver,
        threadId: String,
        limit: Int
    ): List<FfiSmsMessage> {
        val messages = mutableListOf<FfiSmsMessage>()
        var remainingInlineBudget = MMS_INLINE_PAYLOAD_BUDGET_BYTES

        val uri = Telephony.Mms.CONTENT_URI.buildUpon()
            .appendQueryParameter("limit", limit.toString())
            .build()

        val cursor = resolver.query(
            uri,
            arrayOf(
                Telephony.Mms._ID,
                Telephony.Mms.THREAD_ID,
                Telephony.Mms.DATE,
                Telephony.Mms.READ,
                Telephony.Mms.MESSAGE_BOX
            ),
            "${Telephony.Mms.THREAD_ID} = ?",
            arrayOf(threadId),
            "${Telephony.Mms.DATE} DESC"
        )

        cursor?.use {
            val idIndex = it.getColumnIndex(Telephony.Mms._ID)
            val threadIndex = it.getColumnIndex(Telephony.Mms.THREAD_ID)
            val dateIndex = it.getColumnIndex(Telephony.Mms.DATE)
            val readIndex = it.getColumnIndex(Telephony.Mms.READ)
            val msgBoxIndex = it.getColumnIndex(Telephony.Mms.MESSAGE_BOX)

            while (it.moveToNext()) {
                if (idIndex < 0) continue

                val mmsId = it.getString(idIndex) ?: continue
                val thread = if (threadIndex >= 0) it.getString(threadIndex) ?: threadId else threadId
                val rawDate = if (dateIndex >= 0) it.getLong(dateIndex) else 0L
                val timestamp = normalizeMmsTimestamp(rawDate)
                val msgBox = if (msgBoxIndex >= 0) it.getInt(msgBoxIndex) else MMS_MESSAGE_BOX_INBOX
                val read = if (readIndex >= 0) it.getInt(readIndex) == 1 else false
                val isOutgoing = isMmsOutgoing(msgBox)
                val address = getMmsAddress(resolver, mmsId, isOutgoing)
                val content = getMmsContent(resolver, mmsId, remainingInlineBudget)
                remainingInlineBudget = (remainingInlineBudget - content.inlineBytesUsed).coerceAtLeast(0)
                val hasInlineImage = content.attachments.any { attachment ->
                    attachment.contentType.startsWith("image/") && !attachment.data.isNullOrBlank()
                }
                val body = content.body.ifBlank {
                    if (hasInlineImage) "" else getAttachmentPreviewLabel(content.attachments)
                }

                messages.add(
                    FfiSmsMessage(
                        id = "mms:$mmsId",
                        threadId = thread,
                        address = address,
                        contactName = getContactNameForNumber(address),
                        body = body,
                        timestamp = timestamp.toULong(),
                        isOutgoing = isOutgoing,
                        isRead = read,
                        status = mapMmsStatus(msgBox, isOutgoing),
                        attachments = content.attachments
                    )
                )
            }
        }

        return messages
    }

    private fun getLatestIncomingMmsMessage(resolver: ContentResolver): FfiSmsMessage? {
        val uri = Telephony.Mms.CONTENT_URI.buildUpon()
            .appendQueryParameter("limit", "1")
            .build()

        val cursor = resolver.query(
            uri,
            arrayOf(
                Telephony.Mms._ID,
                Telephony.Mms.THREAD_ID,
                Telephony.Mms.DATE,
                Telephony.Mms.READ,
                Telephony.Mms.MESSAGE_BOX
            ),
            "${Telephony.Mms.MESSAGE_BOX} = ?",
            arrayOf(MMS_MESSAGE_BOX_INBOX.toString()),
            "${Telephony.Mms.DATE} DESC"
        )

        cursor?.use {
            val idIndex = it.getColumnIndex(Telephony.Mms._ID)
            val threadIndex = it.getColumnIndex(Telephony.Mms.THREAD_ID)
            val dateIndex = it.getColumnIndex(Telephony.Mms.DATE)
            val readIndex = it.getColumnIndex(Telephony.Mms.READ)

            if (!it.moveToFirst() || idIndex < 0) {
                return null
            }

            val mmsId = it.getString(idIndex) ?: return null
            val threadId = if (threadIndex >= 0) it.getString(threadIndex) ?: "" else ""
            val rawDate = if (dateIndex >= 0) it.getLong(dateIndex) else 0L
            val timestamp = normalizeMmsTimestamp(rawDate)
            val isRead = if (readIndex >= 0) it.getInt(readIndex) == 1 else false
            val address = getMmsAddress(resolver, mmsId, false)
            val content = getMmsContent(resolver, mmsId, MMS_INLINE_PAYLOAD_BUDGET_BYTES)
            val hasInlineImage = content.attachments.any { attachment ->
                attachment.contentType.startsWith("image/") && !attachment.data.isNullOrBlank()
            }
            val body = content.body.ifBlank {
                if (hasInlineImage) "" else getAttachmentPreviewLabel(content.attachments)
            }

            return FfiSmsMessage(
                id = "mms:$mmsId",
                threadId = threadId,
                address = address,
                contactName = getContactNameForNumber(address),
                body = body,
                timestamp = timestamp.toULong(),
                isOutgoing = false,
                isRead = isRead,
                status = SmsStatus.RECEIVED,
                attachments = content.attachments
            )
        }

        return null
    }

    private fun initializeIncomingMmsState(resolver: ContentResolver) {
        lastNotifiedIncomingMmsId = getLatestIncomingMmsMessage(resolver)?.id
    }

    private fun scheduleLatestIncomingMmsDispatch() {
        pendingMmsDispatch?.let(observerHandler::removeCallbacks)

        val runnable = Runnable {
            val latest = getLatestIncomingMmsMessage(context.contentResolver) ?: return@Runnable
            if (latest.id == lastNotifiedIncomingMmsId) {
                return@Runnable
            }

            lastNotifiedIncomingMmsId = latest.id
            listener?.onNewSmsReceived(latest)
        }

        pendingMmsDispatch = runnable
        observerHandler.postDelayed(runnable, 700)
    }

    private fun normalizeMmsTimestamp(rawTimestamp: Long): Long {
        return if (rawTimestamp in 1..999_999_999_999L) {
            rawTimestamp * 1000
        } else {
            rawTimestamp
        }
    }

    private fun isMmsOutgoing(messageBox: Int): Boolean {
        return messageBox == MMS_MESSAGE_BOX_SENT ||
                messageBox == MMS_MESSAGE_BOX_OUTBOX ||
                messageBox == MMS_MESSAGE_BOX_DRAFT
    }

    private fun mapMmsStatus(messageBox: Int, isOutgoing: Boolean): SmsStatus {
        return when {
            !isOutgoing -> SmsStatus.RECEIVED
            messageBox == MMS_MESSAGE_BOX_OUTBOX || messageBox == MMS_MESSAGE_BOX_DRAFT -> SmsStatus.PENDING
            messageBox == MMS_MESSAGE_BOX_FAILED -> SmsStatus.FAILED
            else -> SmsStatus.SENT
        }
    }

    private fun getMmsAddress(resolver: ContentResolver, mmsId: String, isOutgoing: Boolean): String {
        val preferredType = if (isOutgoing) MMS_ADDRESS_TYPE_TO else MMS_ADDRESS_TYPE_FROM
        var fallbackAddress: String? = null

        val cursor = resolver.query(
            Uri.parse("content://mms/$mmsId/addr"),
            arrayOf("address", "type"),
            null,
            null,
            null
        )

        cursor?.use {
            val addressIndex = it.getColumnIndex("address")
            val typeIndex = it.getColumnIndex("type")

            while (it.moveToNext()) {
                val rawAddress = if (addressIndex >= 0) it.getString(addressIndex) else null
                if (rawAddress.isNullOrEmpty() || !isUsableMmsAddress(rawAddress)) {
                    continue
                }

                val type = if (typeIndex >= 0) it.getInt(typeIndex) else -1
                if (type == preferredType) {
                    return rawAddress
                }

                if (fallbackAddress == null) {
                    fallbackAddress = rawAddress
                }
            }
        }

        return fallbackAddress ?: ""
    }

    private data class MmsContent(
        val body: String,
        val attachments: List<FfiMmsAttachment>,
        val inlineBytesUsed: Int
    )

    private fun getMmsPreviewText(resolver: ContentResolver, mmsId: String): String {
        val content = getMmsContent(resolver, mmsId, 0)
        return content.body.ifBlank {
            getAttachmentPreviewLabel(content.attachments)
        }
    }

    private fun getAttachmentPreviewLabel(attachments: List<FfiMmsAttachment>): String {
        val first = attachments.firstOrNull() ?: return ""
        return when {
            first.contentType.startsWith("image/") -> "Photo"
            first.contentType.startsWith("video/") -> "Video"
            first.contentType.startsWith("audio/") -> "Audio"
            else -> "Attachment"
        }
    }

    private fun getMmsContent(
        resolver: ContentResolver,
        mmsId: String,
        inlineBudgetBytes: Int
    ): MmsContent {
        val textParts = mutableListOf<String>()
        val attachments = mutableListOf<FfiMmsAttachment>()
        var remainingInlineBudget = inlineBudgetBytes.coerceAtLeast(0)
        var inlineBytesUsed = 0

        val cursor = resolver.query(
            MMS_PARTS_URI,
            arrayOf("_id", "ct", "_data", "text", "name", "fn", "cl"),
            "mid = ?",
            arrayOf(mmsId),
            null
        )

        cursor?.use {
            val partIdIndex = it.getColumnIndex("_id")
            val contentTypeIndex = it.getColumnIndex("ct")
            val dataIndex = it.getColumnIndex("_data")
            val textIndex = it.getColumnIndex("text")
            val nameIndex = it.getColumnIndex("name")
            val fileNameIndex = it.getColumnIndex("fn")
            val contentLocationIndex = it.getColumnIndex("cl")

            while (it.moveToNext()) {
                val partId = if (partIdIndex >= 0) it.getString(partIdIndex) else null
                if (partId.isNullOrEmpty()) {
                    continue
                }

                val contentType = if (contentTypeIndex >= 0) it.getString(contentTypeIndex) else null
                if (contentType.isNullOrBlank()) {
                    continue
                }

                if (contentType == "application/smil") {
                    continue
                }

                val hasData = dataIndex >= 0 && !it.getString(dataIndex).isNullOrEmpty()

                if (contentType.startsWith("text/")) {
                    val partText = if (hasData) {
                        readMmsPartText(resolver, partId)
                    } else {
                        if (textIndex >= 0) it.getString(textIndex) else null
                    }

                    if (!partText.isNullOrBlank()) {
                        textParts.add(partText)
                    }
                    continue
                }

                val filename = listOf(
                    if (nameIndex >= 0) it.getString(nameIndex) else null,
                    if (fileNameIndex >= 0) it.getString(fileNameIndex) else null,
                    if (contentLocationIndex >= 0) it.getString(contentLocationIndex) else null
                ).firstOrNull { name -> !name.isNullOrBlank() }

                var encodedData: String? = null
                if (contentType.startsWith("image/") && remainingInlineBudget > 0) {
                    val bytes = readMmsPartBytes(resolver, partId, remainingInlineBudget)
                    if (bytes != null && bytes.isNotEmpty()) {
                        encodedData = Base64.getEncoder().encodeToString(bytes)
                        remainingInlineBudget -= bytes.size
                        inlineBytesUsed += bytes.size
                    }
                }

                attachments.add(
                    FfiMmsAttachment(
                        id = "mms:$mmsId:$partId",
                        contentType = contentType,
                        filename = filename,
                        data = encodedData
                    )
                )
            }
        }

        return MmsContent(
            body = textParts.joinToString("\n").trim(),
            attachments = attachments,
            inlineBytesUsed = inlineBytesUsed
        )
    }

    private fun isUsableMmsAddress(address: String): Boolean {
        val normalized = address.trim().lowercase()
        return normalized.isNotEmpty() &&
                normalized != "insert-address-token" &&
                normalized != "undisclosed-recipients" &&
                normalized != "anonymous" &&
                normalized != "unknown"
    }

    private fun readMmsPartBytes(
        resolver: ContentResolver,
        partId: String,
        maxBytes: Int
    ): ByteArray? {
        if (partId.isEmpty() || maxBytes <= 0) {
            return null
        }

        return try {
            resolver.openInputStream(Uri.parse("content://mms/part/$partId"))?.use { input ->
                val output = ByteArrayOutputStream()
                val buffer = ByteArray(8192)
                var totalRead = 0
                var exceededLimit = false

                while (true) {
                    val bytesRead = input.read(buffer)
                    if (bytesRead == -1) {
                        break
                    }
                    totalRead += bytesRead
                    if (totalRead > maxBytes) {
                        exceededLimit = true
                        break
                    }
                    output.write(buffer, 0, bytesRead)
                }

                if (exceededLimit) {
                    null
                } else {
                    output.toByteArray()
                }
            }
        } catch (_: Exception) {
            null
        }
    }

    private fun readMmsPartText(resolver: ContentResolver, partId: String): String? {
        if (partId.isEmpty()) {
            return null
        }

        return try {
            resolver.openInputStream(Uri.parse("content://mms/part/$partId"))
                ?.bufferedReader()
                ?.use { reader -> reader.readText() }
        } catch (_: Exception) {
            null
        }
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
                if (idIndex < 0) continue
                val id = it.getString(idIndex) ?: continue

                val number = if (numberIndex >= 0) it.getString(numberIndex) ?: "" else ""
                val name = if (nameIndex >= 0) it.getString(nameIndex) else null
                val type = if (typeIndex >= 0) it.getInt(typeIndex) else CallLog.Calls.INCOMING_TYPE
                val date = if (dateIndex >= 0) it.getLong(dateIndex) else 0L
                val duration = if (durationIndex >= 0) it.getInt(durationIndex) else 0
                val read = if (readIndex >= 0) it.getInt(readIndex) == 1 else false

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
                data = "tel:$number".toUri()
                flags = Intent.FLAG_ACTIVITY_NEW_TASK
            }
            context.startActivity(intent)
            true
        } catch (_: Exception) {
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
                    try {
                        // Use reflection to avoid deprecation warning for acceptRingingCall
                        val method = telecomManager.javaClass.getMethod("acceptRingingCall")
                        method.invoke(telecomManager)
                        Log.d("TelephonyProvider", "Call answered via TelecomManager")
                        true
                    } catch (e: Exception) {
                        Log.w("TelephonyProvider", "Failed to answer call: ${e.message}")
                        false
                    }
                }

                CallAction.Reject, CallAction.HangUp -> {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                        try {
                            // Use reflection to avoid deprecation warning for endCall
                            val method = telecomManager.javaClass.getMethod("endCall")
                            val result = method.invoke(telecomManager) as? Boolean ?: false
                            Log.d("TelephonyProvider", "Call ended via TelecomManager: $result")
                            result
                        } catch (e: Exception) {
                            Log.w("TelephonyProvider", "Failed to end call: ${e.message}")
                            false
                        }
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

    private fun getLastIncomingCallNumber(): String {
        if (!hasCallLogPermission()) return ""

        try {
            val cursor = context.contentResolver.query(
                CallLog.Calls.CONTENT_URI,
                arrayOf(CallLog.Calls.NUMBER),
                "${CallLog.Calls.TYPE} = ?",
                arrayOf(CallLog.Calls.INCOMING_TYPE.toString()),
                "${CallLog.Calls.DATE} DESC LIMIT 1"
            )

            return cursor?.use {
                if (it.moveToFirst()) {
                    it.getString(0) ?: ""
                } else {
                    ""
                }
            } ?: ""
        } catch (e: Exception) {
            Log.e("TelephonyProvider", "Error getting last call number", e)
            return ""
        }
    }

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
                val index = it.getColumnIndex(ContactsContract.PhoneLookup.DISPLAY_NAME)
                if (index >= 0) {
                    it.getString(index)
                } else {
                    null
                }
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
                                status = SmsStatus.RECEIVED,
                                attachments = emptyList()
                            )
                            listener?.onNewSmsReceived(ffiMessage)
                        }
                    }
                }
            }

            val smsFilter = IntentFilter(Telephony.Sms.Intents.SMS_RECEIVED_ACTION)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                context.registerReceiver(smsReceiver, smsFilter, Context.RECEIVER_NOT_EXPORTED)
            } else {
                context.registerReceiver(smsReceiver, smsFilter)
            }

            initializeIncomingMmsState(context.contentResolver)
            mmsObserver = object : ContentObserver(observerHandler) {
                override fun onChange(selfChange: Boolean, uri: Uri?) {
                    scheduleLatestIncomingMmsDispatch()
                }

                override fun onChange(selfChange: Boolean) {
                    scheduleLatestIncomingMmsDispatch()
                }
            }

            mmsObserver?.let { observer ->
                context.contentResolver.registerContentObserver(
                    Telephony.Mms.CONTENT_URI,
                    true,
                    observer
                )
            }
        }

        // Call State Receiver
        if (hasPhonePermission()) {
            callStateReceiver = object : BroadcastReceiver() {
                override fun onReceive(context: Context?, intent: Intent?) {
                    if (intent?.action == TelephonyManager.ACTION_PHONE_STATE_CHANGED) {
                        val state = intent.getStringExtra(TelephonyManager.EXTRA_STATE)
                        // Use string literal to avoid deprecation warning for EXTRA_INCOMING_NUMBER
                        var number = intent.getStringExtra("incoming_number") ?: ""

                        // Fallback to CallLog if number is missing (Android 9+)
                        if (number.isEmpty() && state == TelephonyManager.EXTRA_STATE_RINGING) {
                            // Small delay might be needed for CallLog to update, but we try anyway
                            number = getLastIncomingCallNumber()
                        }

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
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                context.registerReceiver(callStateReceiver, callFilter, Context.RECEIVER_NOT_EXPORTED)
            } else {
                context.registerReceiver(callStateReceiver, callFilter)
            }
        }
    }

    fun unregisterReceivers() {
        smsReceiver?.let {
            try {
                context.unregisterReceiver(it)
            } catch (_: Exception) {
                // Ignore if not registered
            }
            smsReceiver = null
        }

        callStateReceiver?.let {
            try {
                context.unregisterReceiver(it)
            } catch (_: Exception) {
                // Ignore if not registered
            }
            callStateReceiver = null
        }

        pendingMmsDispatch?.let(observerHandler::removeCallbacks)
        pendingMmsDispatch = null
        mmsObserver?.let { observer ->
            try {
                context.contentResolver.unregisterContentObserver(observer)
            } catch (_: Exception) {
                // Ignore if not registered
            }
        }
        mmsObserver = null
        lastNotifiedIncomingMmsId = null
    }
}
