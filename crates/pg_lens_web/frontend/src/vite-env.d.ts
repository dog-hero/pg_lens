// Vite's `?url` asset imports (used only by the demo entry, src/demo.ts).
// Declared locally instead of pulling in `vite/client`, whose ambient types
// would apply to the production sources too.
declare module "*?url" {
  const url: string;
  export default url;
}
