package com.connected.app

import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.recyclerview.widget.RecyclerView
import com.connected.core.DiscoveredDevice

class DeviceAdapter(
    private val onDeviceClick: (DiscoveredDevice) -> Unit
) : RecyclerView.Adapter<DeviceAdapter.DeviceViewHolder>() {

    private val devices = mutableListOf<DiscoveredDevice>()

    fun addDevice(device: DiscoveredDevice) {
        // Check by ID first
        var existingIndex = devices.indexOfFirst { it.id == device.id }

        // Also check by name+IP to catch devices that restarted with new ID
        if (existingIndex < 0) {
            existingIndex = devices.indexOfFirst { it.name == device.name && it.ip == device.ip }
        }

        // Also check by IP alone (same device, possibly different name)
        if (existingIndex < 0) {
            existingIndex = devices.indexOfFirst { it.ip == device.ip }
        }

        if (existingIndex >= 0) {
            devices[existingIndex] = device
            notifyItemChanged(existingIndex)
        } else {
            devices.add(device)
            notifyItemInserted(devices.size - 1)
        }
    }

    fun removeDevice(deviceId: String) {
        val index = devices.indexOfFirst { it.id == deviceId }
        if (index >= 0) {
            devices.removeAt(index)
            notifyItemRemoved(index)
        }
    }

    fun removeDeviceByIp(ip: String) {
        val index = devices.indexOfFirst { it.ip == ip }
        if (index >= 0) {
            devices.removeAt(index)
            notifyItemRemoved(index)
        }
    }

    fun removeStaleDevices(reachableDevices: List<DiscoveredDevice>) {
        val reachableIds = reachableDevices.map { it.id }.toSet()
        val reachableIps = reachableDevices.map { it.ip }.toSet()

        // Find devices that are no longer in the discovered list
        val toRemove = devices.filter { it.id !in reachableIds && it.ip !in reachableIps }
        toRemove.forEach { device ->
            val index = devices.indexOf(device)
            if (index >= 0) {
                devices.removeAt(index)
                notifyItemRemoved(index)
            }
        }
    }

    fun clear() {
        val size = devices.size
        devices.clear()
        notifyItemRangeRemoved(0, size)
    }

    fun getDevices(): List<DiscoveredDevice> = devices.toList()

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): DeviceViewHolder {
        val view = LayoutInflater.from(parent.context)
            .inflate(R.layout.item_device, parent, false)
        return DeviceViewHolder(view)
    }

    override fun onBindViewHolder(holder: DeviceViewHolder, position: Int) {
        holder.bind(devices[position])
    }

    override fun getItemCount(): Int = devices.size

    inner class DeviceViewHolder(itemView: View) : RecyclerView.ViewHolder(itemView) {
        private val tvName: TextView = itemView.findViewById(R.id.tvDeviceName)
        private val tvDetails: TextView = itemView.findViewById(R.id.tvDeviceDetails)
        private val tvType: TextView = itemView.findViewById(R.id.tvDeviceType)

        fun bind(device: DiscoveredDevice) {
            tvName.text = device.name
            tvDetails.text = "${device.ip}:${device.port}"
            tvType.text = device.deviceType.uppercase()

            itemView.setOnClickListener {
                onDeviceClick(device)
            }
        }
    }
}
