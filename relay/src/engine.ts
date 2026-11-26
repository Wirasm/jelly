// The QuickJS engine verbs run in (verbs.ts). Workers cannot eval, so QuickJS
// runs as WebAssembly, and the .wasm is imported as a module because a Worker
// cannot fetch it at runtime.

import { newQuickJSWASMModuleFromVariant, newVariant, type QuickJSWASMModule } from "quickjs-emscripten-core";
import baseVariant from "@jitl/quickjs-wasmfile-release-sync";
// By path, not the package's "./wasm" export: wrangler picks the wasm loader by file name.
import wasmModule from "../node_modules/@jitl/quickjs-wasmfile-release-sync/dist/emscripten-module.wasm";

const variant = newVariant(baseVariant, { wasmModule });
let loading: Promise<QuickJSWASMModule> | undefined;
export const engine = () => (loading ??= newQuickJSWASMModuleFromVariant(variant));
