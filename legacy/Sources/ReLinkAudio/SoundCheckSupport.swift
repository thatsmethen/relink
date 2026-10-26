import Foundation
import RaveLinkCore

/// Every entry point verifies isolation, including decoded helper commands.
/// Session names and GUI construction are not trusted as a routing boundary.
func validateSoundCheckSession(_ session: MixerSession, channelID: UUID) throws {
    _ = try session.validated()
    guard session.channels.count == 1, session.channels[0].id == channelID,
          session.buses.count == 1, session.buses[0].deviceUID != nil,
          session.buses[0].additionalOutputs.isEmpty, session.sends.count == 1 else {
        throw AudioRoutingError.configuration("Sound Check requires exactly one input group and one explicit listening destination.")
    }
}

func validatedExcludedApplications(_ identifiers: [String]) throws -> [String] {
    var bytes = 0
    for identifier in identifiers {
        let count = identifier.utf8.count
        guard count > 0, count <= 4096,
              !identifier.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains),
              identifier == identifier.trimmingCharacters(in: .whitespacesAndNewlines),
              bytes <= EngineWire.maximumLineBytes - count else {
            throw AudioRoutingError.configuration("An excluded application identifier is invalid or the exclusion list is too large.")
        }
        bytes += count
    }
    return Set(identifiers).sorted()
}
