/**
 * @mwydyn/guest — TypeScript types for mwydyn plugin authors.
 * The runtime (`globalThis.mwydyn`) is injected by the artifact's
 * prelude; this package is TYPES ONLY. Transpile with your own
 * tsc/esbuild, then compose the artifact with `mwydyn-js build`.
 */

/** An address: a string is an INSTANCE id; an object is NAMED. */
export type Address = string | { context: string; name: string };

/** A blob reference (frozen; the runtime brands it). */
export interface BlobRef {
  readonly digest: string;
  readonly mediaType: string;
  readonly size: number;
}

/** The letter your actor receives — payload arrives DECODED. */
export interface Letter<T = unknown> {
  readonly kind: string;
  readonly payload: T;
  readonly cause: string | null;
  readonly correlation: string | null;
  readonly authority: readonly string[];
}

/** What a letter arrives with: the sender + the ambient face. */
export interface Cx {
  sender(): Address;
  blobPut(bytes: Uint8Array, mediaType: string, ttlMicros?: number): BlobRef;
  blobGet(ref: BlobRef): Uint8Array;
  blobLen(ref: BlobRef): number;
  caps(): { verb: string; object: string }[];
}

export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";
export type Instrument = "counter" | "gauge" | "histogram";

/** Effects are RETURNED, never called (the law). */
export declare namespace Effect {
  function post(to: Address, kind: string, payload: unknown): object;
  function reply(letter: Letter, cx: Cx, kind: string, payload: unknown): object;
  /** §9.5: posts to the payload's `replyTo`; null = no routing. */
  function replyVia(letter: Letter, kind: string, payload: unknown): object | null;
  function log(level: LogLevel, message: string, attrs?: object): object;
  function metric(
    name: string,
    instrument: Instrument,
    value: number,
    opts?: { unit?: string; attrs?: object }
  ): object;
}

/** Refusals are OBSERVED by name — never swallowed. */
export declare class MwydynRefusal extends Error {
  readonly code: string;
}

export interface Actor<T = unknown> {
  receive(letter: Letter<T>, cx: Cx): object[];
}

export interface DurableHandlers<TState = unknown> {
  snapshot(): TState;
  restore(state: TState): void;
}

/**
 * THE THREE WORDS. `entry` = stateless; `statefulEntry` = rollback;
 * `durableEntry` = rollback + snapshot/restore. Exactly one, at
 * module top level — the artifact's §9.2 declaration reads it.
 */
declare global {
  // eslint-disable-next-line no-var
  var mwydyn: {
    entry(actor: Actor): void;
    statefulEntry(actor: Actor): void;
    durableEntry(actor: Actor, durable: DurableHandlers): void;
    Effect: typeof Effect;
    MwydynRefusal: typeof MwydynRefusal;
    fail(message?: string): never;
  };
}
