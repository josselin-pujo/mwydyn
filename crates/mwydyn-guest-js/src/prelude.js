// THE PRELUDE (round 43): the entire runtime API a JS/TS plugin
// sees — the three words, Effect builders (post/reply/replyVia/
// log/metric), the Cx surface (sender + blobs + caps), refusal
// objects. ALL runtime code lives here (types-only npm package,
// work-notes/43 fork 1); the wrapper injects this before the
// author's script. The `__mw*` intrinsics are the wrapper's.
(function () {
  "use strict";

  const registry = { mode: 0, actor: null, durable: null };

  function addr(a) {
    if (typeof a === "string") return a; // Instance
    if (a && typeof a === "object" && "context" in a) return { context: a.context, name: a.name };
    throw new Error("address: string (instance) or {context, name}");
  }

  function mkLetter(kind, payload, cause, correlation, authority) {
    return {
      kind: kind,
      payload: payload === undefined ? null : payload,
      cause: cause === undefined ? null : cause,
      correlation: correlation === undefined ? null : correlation,
      authority: authority === undefined ? [] : authority,
    };
  }

  const Effect = {
    post(to, kind, payload) {
      return { post: { to: addr(to), letter: mkLetter(kind, payload) } };
    },
    reply(letter, cx, kind, payload) {
      return { post: { to: addr(cx.sender()), letter: mkLetter(kind, payload, null, letter.correlation) } };
    },
    replyVia(letter, kind, payload) {
      const p = letter.payload;
      const to = p && typeof p === "object" && p.replyTo !== undefined ? p.replyTo : null;
      if (to === null) return null; // §9.5: no routing — caller falls back
      return { post: { to: addr(to), letter: mkLetter(kind, payload, null, letter.correlation) } };
    },
    // §3.6 telemetry — best-effort, lossy by design, never strikes
    // the DECLARE form (§3.6): {"kind": K, K: {fields}} — payload
    // nested under the kind key, never flat
    log(level, message, attrs) {
      return {
        emit: {
          kind: "log",
          log: { level: level, message: message, ...(attrs ? { attrs } : {}) },
        },
      };
    },
    metric(name, instrument, value, opts) {
      const o = opts || {};
      return {
        emit: {
          kind: "metric",
          metric: {
            name: name,
            instrument: instrument,
            value: value,
            ...(o.unit ? { unit: o.unit } : {}),
            ...(o.attrs ? { attrs: o.attrs } : {}),
          },
        },
      };
    },
  };

  class MwydynRefusal extends Error {
    constructor(code) {
      super("mwydyn refusal: " + code);
      this.code = code;
    }
  }

  // the ambient bridge: __mwAmbient(requestObj) -> result value,
  // throws {code} on refusal (the wrapper refuses by name)
  function ambient(request) {
    const r = __mwAmbient(request);
    if (r !== null && typeof r === "object" && "__refused" in r) {
      throw new MwydynRefusal(r.__refused);
    }
    return r;
  }

  function mkCx(senderAddr) {
    return {
      sender: () => senderAddr,
      blobPut(bytes, mediaType, ttlMicros) {
        return ambient({
          verb: "blob.put",
          bytes: bytes,
          mediaType: mediaType,
          ttl: ttlMicros === undefined ? null : ttlMicros,
        });
      },
      blobGet(ref) {
        return ambient({ verb: "blob.get", ref: ref });
      },
      blobLen(ref) {
        return ambient({ verb: "blob.len", ref: ref });
      },
      caps() {
        return ambient({ verb: "caps" }) || [];
      },
    };
  }

  globalThis.mwydyn = {
    entry(actor) {
      registry.mode = 0;
      registry.actor = actor;
    },
    statefulEntry(actor) {
      registry.mode = 1;
      registry.actor = actor;
    },
    durableEntry(actor, durable) {
      registry.mode = 3;
      registry.actor = actor;
      registry.durable = durable;
    },
    Effect: Effect,
    MwydynRefusal: MwydynRefusal,
    fail(message) {
      throw new Error(message === undefined ? "mwydyn: fail" : message);
    },
    __mode() {
      return registry.mode;
    },
    __actor() {
      return registry.actor;
    },
    __durable() {
      return registry.durable;
    },
    __mkCx: mkCx,
  };
})();
