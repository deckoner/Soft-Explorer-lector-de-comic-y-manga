/// <reference types="vite/client" />

declare module "*.svg" {
  const assetUrl: string;
  export default assetUrl;
}
