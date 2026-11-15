// Mobile Wallet Adapter's JS library (@solana-mobile/mobile-wallet-adapter-protocol,
// 3.0.1 and the source after it) turns a JSON-RPC error into
// SolanaMobileWalletAdapterProtocolError with its code and message, and drops
// its `data`. Airlock puts the refusal's code there ({ "code": "unsupported",
// "requirement": "delivery" }, docs/intents.md), so the page could not tell one
// refusal from another. This hands `data` to the error's constructor, which
// already takes it as its fourth argument.
//
// build.mjs applies it to the library as esbuild loads it. A library whose code
// no longer reads as expected fails the build, rather than losing `data` quietly.

export const MWA_LIBRARY = /[\\/]@solana-mobile[\\/]mobile-wallet-adapter-protocol[\\/]lib[\\/]esm[\\/]index\.browser\.js$/;

const DROPS_DATA = "new SolanaMobileWalletAdapterProtocolError(jsonRpcMessage.id, jsonRpcMessage.error.code, jsonRpcMessage.error.message)";
const KEEPS_DATA = "new SolanaMobileWalletAdapterProtocolError(jsonRpcMessage.id, jsonRpcMessage.error.code, jsonRpcMessage.error.message, jsonRpcMessage.error.data)";

/** The library's source with `data` kept. Throws when the line is not there once. */
export function keepErrorData(source) {
  const count = source.split(DROPS_DATA).length - 1;
  if (count !== 1) {
    throw new Error(
      `mwa-patch: expected the line that builds a JSON-RPC error once in the MWA library, found it ${count} times. ` +
        "Check whether the library keeps error.data now, and update or drop mwa-patch.mjs.",
    );
  }
  return source.replace(DROPS_DATA, KEEPS_DATA);
}

/** An esbuild plugin that applies keepErrorData to the library. */
export function mwaErrorDataPlugin() {
  return {
    name: "mwa-error-data",
    setup(build) {
      build.onLoad({ filter: MWA_LIBRARY }, async (args) => {
        const { readFile } = await import("node:fs/promises");
        return { contents: keepErrorData(await readFile(args.path, "utf8")), loader: "js" };
      });
    },
  };
}
