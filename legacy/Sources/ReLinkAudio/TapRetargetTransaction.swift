/// Control-thread ordering shared by real HAL retargeting and fault tests. A
/// failure leaves the caller's independent output gate closed; only the actor
/// that still owns this graph generation may reopen it after successful return.
func performTapRetargetTransaction<Node>(nodes: [Node], checkCancellation: () throws -> Void,
    stop: (Node) throws -> Void, apply: (Node) throws -> Void,
    flush: () -> Void, start: (Node) throws -> Void) throws {
    for node in nodes { try checkCancellation(); try stop(node) }
    for node in nodes { try checkCancellation(); try apply(node) }
    try checkCancellation(); flush()
    for node in nodes { try checkCancellation(); try start(node) }
    try checkCancellation()
}
