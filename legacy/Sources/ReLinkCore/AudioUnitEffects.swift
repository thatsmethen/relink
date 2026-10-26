import Foundation

public struct AudioUnitComponentID: Codable, Hashable, Sendable {
    public let type: UInt32
    public let subtype: UInt32
    public let manufacturer: UInt32
    public init(type: UInt32, subtype: UInt32, manufacturer: UInt32) {
        self.type = type; self.subtype = subtype; self.manufacturer = manufacturer
    }
    public var key: String { String(format: "%08x:%08x:%08x", type, subtype, manufacturer) }
}
public struct AudioUnitDescriptor: Codable, Equatable, Identifiable, Sendable {
    public let component: AudioUnitComponentID
    public let name: String
    public let version: UInt32?
    public let flags: UInt32
    public var id: String { component.key }
    public init(component: AudioUnitComponentID, name: String, version: UInt32?, flags: UInt32 = 0) {
        self.component = component; self.name = name; self.version = version; self.flags = flags
    }
}
public struct AudioUnitParameterSetting: Codable, Equatable, Sendable {
    public let address: UInt64
    public let value: Float
    public init(address: UInt64, value: Float) { self.address = address; self.value = value }
}

/// Binary property-list fullState, bound to one Audio Unit identity and version.
/// The app stores these bytes without unarchiving arbitrary plug-in objects.
public struct AudioUnitState: Codable, Equatable, Sendable {
    public let formatVersion: Int
    public let component: AudioUnitComponentID
    public let componentVersion: UInt32?
    public let propertyList: Data
    public init(component: AudioUnitComponentID, componentVersion: UInt32?, propertyList: Data) {
        self.formatVersion = 1; self.component = component; self.componentVersion = componentVersion; self.propertyList = propertyList
    }
    public func isValid(for descriptor: AudioUnitDescriptor) -> Bool {
        formatVersion == 1 && component == descriptor.component && !propertyList.isEmpty && propertyList.count <= 256 * 1024 &&
        (componentVersion == nil || componentVersion == descriptor.version)
    }
}

public struct AudioUnitEffectSlot: Codable, Equatable, Identifiable, Sendable {
    public var id: UUID
    public var descriptor: AudioUnitDescriptor
    public var state: AudioUnitState?
    public var parameters: [AudioUnitParameterSetting]
    public var isBypassed: Bool
    public init(id: UUID = UUID(), descriptor: AudioUnitDescriptor, state: AudioUnitState? = nil,
                parameters: [AudioUnitParameterSetting] = [], isBypassed: Bool = false) {
        self.id = id; self.descriptor = descriptor; self.state = state
        self.parameters = parameters; self.isBypassed = isBypassed
    }
}
