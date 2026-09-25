// Build-time stub for ink's optional `react-devtools-core` import.
// ink only loads ./devtools.js when DEV=true; we never set it, so this
// module is bundled but never executed. Keeps `bun build --compile` happy
// without shipping a dev-only dependency.
const devtools = {
  connectToDevTools() {},
};
export default devtools;
