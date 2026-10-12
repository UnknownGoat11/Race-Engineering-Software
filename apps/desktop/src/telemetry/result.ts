/** Success value or typed error, mirroring Rust's `Result` for operations whose failure is expected data. */
export type Result<T, E> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly error: E };

/** A successful `Result`. */
export function ok<T>(value: T): { readonly ok: true; readonly value: T } {
  return { ok: true, value };
}

/** A failed `Result`. */
export function err<E>(error: E): { readonly ok: false; readonly error: E } {
  return { ok: false, error };
}
