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

  // ------------------------------------------------------------------
  // THE CANONICAL CBOR CODEC (round 49) — one canon, both realms.
  // The JS realm's mwydyn-data: `decode` is LIBERAL within the
  // restricted type set (non-shortest heads accepted, map keys
  // sorted, f16/f32 widened to f64 — normalize, never reject valid
  // CBOR); `encode` is STRICT canonical (shortest ints, definite
  // lengths, always-f64 floats, keys sorted by (byte-length, bytes)).
  // Refusals carry `.code` — the corpus kebab names.
  //
  // The JS value model mirrors the barrier's conversion table:
  // number-when-safe (±2^53), BigInt beyond; bytes are Uint8Array;
  // BlobRef arrives branded (`__mwRef` — the same brand the host
  // sets, so a decoded ref IS a host ref). Two JS-realm facts the
  // table already documents, restated: bare integral numbers ARE
  // ints (floatness is explicit — mwydyn.float), and the timestamp
  // tag survives only in its box (mwydyn.timestamp / decode of
  // tag 1001 — bare µs numbers lose the tag at the barrier).

  const SAFE_N = 9007199254740991; // 2^53 - 1
  const SAFE_B = 9007199254740991n;
  const I64_MIN_B = -9223372036854775808n;
  const I64_MAX_B = 9223372036854775807n;
  const MAX_DEPTH = 64;

  function cborError(code, message) {
    const e = new Error(message === undefined ? code : message);
    e.code = code;
    return e;
  }

  function hygienicKey(key) {
    if (
      key === "__proto__" ||
      key === "constructor" ||
      key === "prototype" ||
      key.length === 0
    ) {
      return false;
    }
    let c = key.charCodeAt(0);
    if (!((c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c === 95)) return false;
    for (let i = 1; i < key.length; i++) {
      c = key.charCodeAt(i);
      if (
        !((c >= 65 && c <= 90) || (c >= 97 && c <= 122) || (c >= 48 && c <= 57) || c === 95 || c === 45)
      ) {
        return false;
      }
    }
    return true;
  }

  // Canonical key order: (byte-length, bytes). Hygienic keys are
  // ASCII, so UTF-16 code-unit order IS byte order.
  function keyBefore(a, b) {
    if (a.length !== b.length) return a.length - b.length;
    return a < b ? -1 : a > b ? 1 : 0;
  }

  function isCont(b) {
    return (b & 0xc0) === 0x80;
  }

  function utf8Decode(u8) {
    let s = "";
    let i = 0;
    const n = u8.length;
    while (i < n) {
      const b = u8[i];
      if (b < 0x80) {
        s += String.fromCharCode(b);
        i += 1;
      } else if (b >= 0xc2 && b <= 0xdf) {
        if (i + 1 >= n || !isCont(u8[i + 1])) throw cborError("malformed", "bad utf8");
        s += String.fromCharCode(((b & 0x1f) << 6) | (u8[i + 1] & 0x3f));
        i += 2;
      } else if (b >= 0xe0 && b <= 0xef) {
        if (i + 2 >= n || !isCont(u8[i + 1]) || !isCont(u8[i + 2])) {
          throw cborError("malformed", "bad utf8");
        }
        const c1 = u8[i + 1] & 0x3f;
        if ((b === 0xe0 && c1 < 0x20) || (b === 0xed && c1 >= 0x20)) {
          throw cborError("malformed", "bad utf8"); // overlong / surrogate
        }
        s += String.fromCharCode(((b & 0x0f) << 12) | (c1 << 6) | (u8[i + 2] & 0x3f));
        i += 3;
      } else if (b >= 0xf0 && b <= 0xf4) {
        if (i + 3 >= n || !isCont(u8[i + 1]) || !isCont(u8[i + 2]) || !isCont(u8[i + 3])) {
          throw cborError("malformed", "bad utf8");
        }
        const c1 = u8[i + 1] & 0x3f;
        if ((b === 0xf0 && c1 < 0x10) || (b === 0xf4 && c1 > 0x0f)) {
          throw cborError("malformed", "bad utf8"); // overlong / > U+10FFFF
        }
        const cp =
          ((b & 0x07) << 18) | (c1 << 12) | ((u8[i + 2] & 0x3f) << 6) | (u8[i + 3] & 0x3f);
        s += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
        i += 4;
      } else {
        throw cborError("malformed", "bad utf8"); // 0x80..0xc1, 0xf5..0xff
      }
    }
    return s;
  }

  function utf8Bytes(s) {
    const out = [];
    for (let i = 0; i < s.length; i++) {
      const c = s.charCodeAt(i);
      if (c < 0x80) {
        out.push(c);
      } else if (c < 0x800) {
        out.push(0xc0 | (c >> 6), 0x80 | (c & 0x3f));
      } else if (c >= 0xd800 && c <= 0xdbff) {
        const lo = i + 1 < s.length ? s.charCodeAt(i + 1) : 0;
        if (!(lo >= 0xdc00 && lo <= 0xdfff)) {
          throw cborError("malformed", "lone surrogate in text");
        }
        const cp = 0x10000 + ((c - 0xd800) << 10) + (lo - 0xdc00);
        out.push(
          0xf0 | (cp >> 18),
          0x80 | ((cp >> 12) & 0x3f),
          0x80 | ((cp >> 6) & 0x3f),
          0x80 | (cp & 0x3f)
        );
        i += 1;
      } else if (c >= 0xdc00 && c <= 0xdfff) {
        throw cborError("malformed", "lone surrogate in text");
      } else {
        out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
      }
    }
    return out;
  }

  // The brands — the SAME `__mwRef` the host's conversion table sets,
  // so a decoded ref crosses the barrier as a real BlobRef.
  function brand(o, name) {
    Object.defineProperty(o, name, { value: true, enumerable: false });
    return o;
  }

  function mkRef(digest, mediaType, size) {
    return brand({ digest: digest, mediaType: mediaType, size: size }, "__mwRef");
  }

  function mkTs(micros) {
    return brand({ micros: micros }, "__mwTs");
  }

  function brandOf(v) {
    if (v === null || typeof v !== "object" || Array.isArray(v) || v instanceof Uint8Array) {
      return null;
    }
    if (v.__mwRef === true) return "ref";
    if (v.__mwTs === true) return "ts";
    if (v.__mwFloat === true) return "float";
    return null;
  }

  function decodeCbor(bytes) {
    if (!(bytes instanceof Uint8Array)) {
      throw new TypeError("mwydyn.decode: Uint8Array required");
    }
    if (bytes.length === 0) throw cborError("empty");
    let pos = 0;

    function u8() {
      if (pos >= bytes.length) throw cborError("malformed", "truncated");
      const b = bytes[pos];
      pos += 1;
      return b;
    }

    function take(n) {
      if (n > bytes.length - pos) throw cborError("malformed", "truncated");
      const s = bytes.subarray(pos, pos + n);
      pos += n;
      return s;
    }

    // (major, additional-info, argument). The argument is a Number
    // while exactly representable, a BigInt beyond 2^53 (u64 heads).
    function head() {
      const ib = u8();
      const major = ib >>> 5;
      const ai = ib & 0x1f;
      let arg;
      if (ai <= 23) {
        arg = ai;
      } else if (ai === 24) {
        arg = u8();
      } else if (ai === 25) {
        arg = (u8() << 8) | u8();
      } else if (ai === 26) {
        arg = ((u8() << 24) | (u8() << 16) | (u8() << 8) | u8()) >>> 0;
      } else if (ai === 27) {
        const hi = ((u8() << 24) | (u8() << 16) | (u8() << 8) | u8()) >>> 0;
        const lo = ((u8() << 24) | (u8() << 16) | (u8() << 8) | u8()) >>> 0;
        const n = hi * 4294967296 + lo;
        arg = n <= SAFE_N ? n : (BigInt(hi) << 32n) | BigInt(lo);
      } else if (ai <= 30) {
        throw cborError("malformed", "reserved additional info");
      } else {
        throw cborError("indefinite"); // lengths and break — both ways forbidden
      }
      return { major: major, ai: ai, arg: arg };
    }

    function lenArg(arg) {
      if (typeof arg === "bigint" || !Number.isInteger(arg) || arg < 0) {
        throw cborError("malformed", "truncated"); // an absurd length
      }
      return arg;
    }

    // A decoded integer: number while |v| ≤ 2^53, BigInt beyond
    // (the barrier's own boundary — fork 3, ratified round 49).
    function intShape(v) {
      if (v >= -SAFE_B && v <= SAFE_B) return Number(v);
      return v;
    }

    // The (possibly negated) integer a head carries. NO i64 bound
    // here: major 0/1 beyond i64 decode as bignums, validly — the
    // bound is the TIMESTAMP payload's rule, checked at its branch.
    function headInt(h) {
      const v = typeof h.arg === "bigint" ? h.arg : BigInt(h.arg);
      return h.major === 0 ? v : -1n - v;
    }

    const fdv = new DataView(new ArrayBuffer(8));

    // Floats: the head's ARG already carries the raw bits (the
    // reader consumed them) — decode from the argument, never from
    // the byte stream.
    function readFloat(h) {
      let x;
      if (h.ai === 25) {
        const bits = h.arg; // u16
        const sign = bits >>> 15;
        const exp = (bits >> 10) & 0x1f;
        const frac = bits & 0x3ff;
        if (exp === 0) {
          x = frac * Math.pow(2, -24);
        } else if (exp === 31) {
          x = frac === 0 ? Infinity : NaN;
        } else {
          x = (frac / 1024 + 1) * Math.pow(2, exp - 15);
        }
        if (sign === 1) x = -x;
      } else if (h.ai === 26) {
        fdv.setUint32(0, h.arg, false);
        x = fdv.getFloat32(0);
      } else {
        if (typeof h.arg === "number") {
          const hi = Math.floor(h.arg / 4294967296);
          const lo = h.arg - hi * 4294967296;
          fdv.setUint32(0, hi, false);
          fdv.setUint32(4, lo, false);
        } else {
          fdv.setBigUint64(0, h.arg, false);
        }
        x = fdv.getFloat64(0);
      }
      if (!Number.isFinite(x)) throw cborError("non-finite");
      return x;
    }

    function validDigest(d) {
      if (d.length !== 64) return false;
      for (let i = 0; i < 64; i++) {
        const c = d.charCodeAt(i);
        const ok = (c >= 48 && c <= 57) || (c >= 97 && c <= 102);
        if (!ok) return false;
      }
      return true;
    }

    function readValue(depth) {
      if (depth > MAX_DEPTH) throw cborError("too-deep");
      const h = head();
      switch (h.major) {
        case 0:
        case 1: {
          const v = headInt(h);
          return intShape(v);
        }
        case 2:
          return new Uint8Array(take(lenArg(h.arg)));
        case 3:
          return utf8Decode(take(lenArg(h.arg)));
        case 4: {
          const n = lenArg(h.arg);
          const items = new Array(n);
          for (let i = 0; i < n; i++) items[i] = readValue(depth + 1);
          return items;
        }
        case 5: {
          const n = lenArg(h.arg);
          const pairs = [];
          for (let i = 0; i < n; i++) {
            const k = readValue(depth + 1);
            if (typeof k !== "string") throw cborError("malformed", "non-string map key");
            if (!hygienicKey(k)) throw cborError("bad-key");
            pairs.push([k, readValue(depth + 1)]);
          }
          pairs.sort(function (a, b) {
            return keyBefore(a[0], b[0]);
          });
          for (let i = 1; i < pairs.length; i++) {
            if (pairs[i][0] === pairs[i - 1][0]) throw cborError("duplicate-key");
          }
          const o = {};
          for (let i = 0; i < pairs.length; i++) o[pairs[i][0]] = pairs[i][1];
          return o;
        }
        case 6: {
          const tag = h.arg;
          if (tag === 2 || tag === 3) {
            const ph = head();
            if (ph.major !== 2) throw cborError("malformed", "bignum payload not bytes");
            const mag = take(lenArg(ph.arg));
            if (mag.length === 0 || mag[0] === 0) throw cborError("bad-bignum");
            let v = 0n;
            for (let i = 0; i < mag.length; i++) v = (v << 8n) | BigInt(mag[i]);
            return tag === 2 ? v : -1n - v;
          }
          if (tag === 1001) {
            const ph = head();
            if (
              (ph.major !== 0 && ph.major !== 1) ||
              (typeof ph.arg === "bigint" && ph.arg > I64_MAX_B)
            ) {
              throw cborError("malformed", "timestamp payload not an int");
            }
            return mkTs(intShape(headInt(ph)));
          }
          if (tag === 1002) {
            const inner = readValue(depth + 1); // full rules — hygiene, sort, dups
            if (brandOf(inner) !== null || inner === null || typeof inner !== "object") {
              throw cborError("bad-ref");
            }
            const keys = Object.keys(inner); // decode-sorted: size < digest < mediaType
            if (
              keys.length !== 3 ||
              keys[0] !== "size" ||
              keys[1] !== "digest" ||
              keys[2] !== "mediaType"
            ) {
              throw cborError("bad-ref");
            }
            const size = inner.size;
            const digest = inner.digest;
            const mt = inner.mediaType;
            if (typeof digest !== "string" || typeof mt !== "string" || mt.length === 0) {
              throw cborError("bad-ref");
            }
            if (typeof size === "number") {
              if (!Number.isInteger(size) || size < 0) throw cborError("bad-ref");
            } else if (typeof size === "bigint") {
              if (size < 0n || size > 0xffffffffffffffffn) throw cborError("bad-ref");
            } else {
              throw cborError("bad-ref");
            }
            if (!validDigest(digest)) throw cborError("bad-ref");
            return mkRef(digest, mt, size);
          }
          throw cborError("unknown-tag", "tag " + String(tag));
        }
        case 7: {
          if (h.ai === 20) return false;
          if (h.ai === 21) return true;
          if (h.ai === 22) return null;
          if (h.ai === 25 || h.ai === 26 || h.ai === 27) return readFloat(h);
          throw cborError("malformed", "simple value not in the data model");
        }
      }
    }

    const v = readValue(0);
    if (pos !== bytes.length) throw cborError("trailing");
    return v;
  }

  function encodeCbor(value) {
    const out = [];
    const fbuf = new ArrayBuffer(8);
    const fdv = new DataView(fbuf);

    function wHead(major, arg) {
      // arg: Number ≥ 0, ≤ 2^53 — the shortest form
      const m = major << 5;
      if (arg <= 23) {
        out.push(m | arg);
      } else if (arg <= 0xff) {
        out.push(m | 24, arg);
      } else if (arg <= 0xffff) {
        out.push(m | 25, (arg >> 8) & 0xff, arg & 0xff);
      } else if (arg <= 0xffffffff) {
        out.push(m | 26, (arg >>> 24) & 0xff, (arg >>> 16) & 0xff, (arg >>> 8) & 0xff, arg & 0xff);
      } else {
        // arg in (2^32, 2^53] — exact as two 32-bit halves
        const hi = Math.floor(arg / 4294967296);
        const lo = arg - hi * 4294967296;
        out.push(m | 27);
        out.push((hi >>> 24) & 0xff, (hi >>> 16) & 0xff, (hi >>> 8) & 0xff, hi & 0xff);
        out.push((lo >>> 24) & 0xff, (lo >>> 16) & 0xff, (lo >>> 8) & 0xff, lo & 0xff);
      }
    }

    function writeIntVal(v) {
      // v: safe-integer Number or i64-range BigInt
      const neg = v < 0;
      const arg = neg ? (typeof v === "bigint" ? -1n - v : -1 - v) : v;
      if (typeof arg === "number" || arg <= SAFE_B) {
        wHead(neg ? 1 : 0, typeof arg === "number" ? arg : Number(arg));
      } else {
        // arg in (2^53, 2^63] — the 8-byte form
        out.push(((neg ? 1 : 0) << 5) | 27);
        fdv.setBigUint64(0, arg, false);
        for (let i = 0; i < 8; i++) out.push(fdv.getUint8(i));
      }
    }

    function magBytes(v) {
      const hex = v.toString(16);
      const pad = hex.length % 2 === 1 ? "0" + hex : hex;
      const b = [];
      for (let i = 0; i < pad.length; i += 2) {
        b.push(parseInt(pad.substring(i, i + 2), 16));
      }
      return b; // toString(16) of v > 0n is minimal — no leading zeros
    }

    function writeBig(v) {
      const pos = v >= 0n;
      const mag = magBytes(pos ? v : -1n - v);
      wHead(6, pos ? 2 : 3);
      wHead(2, mag.length);
      for (let i = 0; i < mag.length; i++) out.push(mag[i]);
    }

    function pushF64(n) {
      fdv.setFloat64(0, n, false);
      out.push(0xfb);
      for (let i = 0; i < 8; i++) out.push(fdv.getUint8(i));
    }

    function writeValue(v) {
      if (v === null || v === undefined) {
        out.push(0xf6);
      } else if (typeof v === "boolean") {
        out.push(v ? 0xf5 : 0xf4);
      } else if (typeof v === "number") {
        if (!Number.isFinite(v)) throw cborError("non-finite");
        if (Number.isInteger(v) && Math.abs(v) <= SAFE_N) {
          writeIntVal(v);
        } else {
          pushF64(v);
        }
      } else if (typeof v === "bigint") {
        if (v >= I64_MIN_B && v <= I64_MAX_B) {
          writeIntVal(v);
        } else {
          writeBig(v);
        }
      } else if (typeof v === "string") {
        const b = utf8Bytes(v);
        wHead(3, b.length);
        for (let i = 0; i < b.length; i++) out.push(b[i]);
      } else if (v instanceof Uint8Array) {
        wHead(2, v.length);
        for (let i = 0; i < v.length; i++) out.push(v[i]);
      } else if (Array.isArray(v)) {
        wHead(4, v.length);
        for (let i = 0; i < v.length; i++) writeValue(v[i]);
      } else if (typeof v === "object") {
        const b = brandOf(v);
        if (b === "float") {
          if (typeof v.value !== "number" || !Number.isFinite(v.value)) {
            throw cborError("non-finite");
          }
          pushF64(v.value);
        } else if (b === "ts") {
          const us = v.micros;
          const ok =
            (typeof us === "number" && Number.isInteger(us) && Math.abs(us) <= SAFE_N) ||
            (typeof us === "bigint" && us >= I64_MIN_B && us <= I64_MAX_B);
          if (!ok) throw cborError("malformed", "timestamp micros not an i64");
          wHead(6, 1001);
          writeIntVal(us);
        } else if (b === "ref") {
          // A serializer, not a validator: forged-but-shaped refs
          // encode (the barrier validates on crossing; decode
          // validates on reading — this layer refuses only the
          // structurally impossible).
          const digest = v.digest;
          const mt = v.mediaType;
          const size = v.size;
          if (typeof digest !== "string" || typeof mt !== "string") {
            throw cborError("bad-ref", "ref digest/mediaType must be text");
          }
          if (
            !(typeof size === "number" && Number.isInteger(size) && size >= 0) &&
            !(typeof size === "bigint" && size >= 0n)
          ) {
            throw cborError("bad-ref", "ref size must be an unsigned integer");
          }
          wHead(6, 1002);
          // {size, digest, mediaType} in canonical order — every
          // entry is KEY then value (the round-49 harness caught the
          // first draft writing bare values under the map head).
          const keyText = function (k) {
            const kb = utf8Bytes(k);
            wHead(3, kb.length);
            for (let j = 0; j < kb.length; j++) out.push(kb[j]);
          };
          wHead(5, 3);
          keyText("size");
          if (typeof size === "number" || size <= I64_MAX_B) {
            writeIntVal(size);
          } else {
            writeBig(size);
          }
          keyText("digest");
          const d = utf8Bytes(digest);
          wHead(3, d.length);
          for (let i = 0; i < d.length; i++) out.push(d[i]);
          keyText("mediaType");
          const m = utf8Bytes(mt);
          wHead(3, m.length);
          for (let i = 0; i < m.length; i++) out.push(m[i]);
        } else {
          const keys = Object.keys(v); // own enumerable — the barrier's rule
          for (let i = 0; i < keys.length; i++) {
            if (!hygienicKey(keys[i])) throw cborError("bad-key", "key: " + keys[i]);
          }
          keys.sort(keyBefore);
          wHead(5, keys.length);
          for (let i = 0; i < keys.length; i++) {
            const kb = utf8Bytes(keys[i]);
            wHead(3, kb.length);
            for (let j = 0; j < kb.length; j++) out.push(kb[j]);
            writeValue(v[keys[i]]);
          }
        }
      } else {
        throw cborError("malformed", "value not in the data model: " + typeof v);
      }
    }

    writeValue(value);
    return new Uint8Array(out);
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
    // round 49 — the canon codec (see the codec block above)
    decode: decodeCbor,
    encode: encodeCbor,
    float(n) {
      if (typeof n !== "number" || !Number.isFinite(n)) {
        throw new TypeError("mwydyn.float: finite number required");
      }
      return brand({ value: n }, "__mwFloat");
    },
    timestamp(micros) {
      const ok =
        (typeof micros === "number" && Number.isInteger(micros) && Math.abs(micros) <= SAFE_N) ||
        (typeof micros === "bigint" && micros >= I64_MIN_B && micros <= I64_MAX_B);
      if (!ok) throw new TypeError("mwydyn.timestamp: i64 micros required");
      return mkTs(micros);
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
