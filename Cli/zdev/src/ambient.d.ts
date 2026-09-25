// Ambient module declarations for bun text imports (`with { type: "text" }`).
declare module "*.md" {
  const text: string;
  export default text;
}
