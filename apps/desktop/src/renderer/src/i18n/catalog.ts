/** English catalogs are typed against the Chinese shape: a missing key is a compile error. */
export type CatalogShape<T> = {
  [K in keyof T]: T[K] extends (...args: infer A) => string
    ? (...args: A) => string
    : T[K] extends string ? string : CatalogShape<T[K]>
}
