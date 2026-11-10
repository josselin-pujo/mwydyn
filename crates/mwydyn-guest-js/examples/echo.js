// THE STATELESS EXAMPLE (round 43): echo — every letter's payload
// back to its replyTo (§9.5), a telemetry log along for the ride
// (§3.6), the sender as fallback routing.
mwydyn.entry({
  receive(letter, cx) {
    const effects = [
      mwydyn.Effect.log("info", "echo saw " + letter.kind),
    ];
    const via = mwydyn.Effect.replyVia(letter, "demo.echoed@1", letter.payload);
    if (via !== null) {
      effects.push(via);
    } else {
      effects.push(mwydyn.Effect.reply(letter, cx, "demo.echoed@1", letter.payload));
    }
    return effects;
  },
});
