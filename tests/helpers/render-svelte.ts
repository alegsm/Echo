import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { compile, compileModule } from 'svelte/compiler';

type Renderer = (props?: Record<string, unknown>) => string;
const rootDirectory = fileURLToPath(new URL('../../', import.meta.url));

// Bundle intact components with Svelte's public compiler and server renderer.
export function compileRenderer(component: URL): Promise<Renderer>;
export function compileRenderer(source: string): Promise<Renderer>;
export async function compileRenderer(component: string | URL): Promise<Renderer> {
  return compileComponent(component, false);
}

export function compileClient(component: string | URL) { return compileComponent(component, true); }

// Absolute repository source path usable inside generated import specifiers on every host.
export function sourcePath(fromRoot: string): string {
  return join(rootDirectory, fromRoot).replaceAll('\\', '/');
}

async function compileComponent(component: string | URL, client: boolean): Promise<any> {
  const componentPath = component instanceof URL ? fileURLToPath(component) : undefined;
  const resolveDir = componentPath ? dirname(componentPath) : rootDirectory;
  const result = await Bun.build({
    entrypoints: ['echo-render-test'], target: 'bun', conditions: client ? ['svelte', 'browser'] : ['svelte'], throw: false,
    plugins: [{ name: 'render-svelte', setup(build) {
      // Plugin-loaded sources need explicit package-scope lookup on a cold Bun resolver.
      build.onResolve({ filter: /^#/ }, ({ path, resolveDir }) => ({
        path: Bun.resolveSync(path, resolveDir)
      }));
      build.onResolve({ filter: /^echo-render-test$/ }, () => ({ path: 'entry', namespace: 'render-test' }));
      build.onResolve({ filter: /^echo-component-test$/ }, () => componentPath
        ? { path: componentPath }
        : { path: 'component', namespace: 'render-test' });
      build.onLoad({ filter: /.*/, namespace: 'render-test' }, ({ path }) => ({
        contents: path === 'entry'
          ? client ? "import { mount, unmount, flushSync } from 'svelte'; import Component from 'echo-component-test'; export default { Component, mount, unmount, flushSync };" : "import { render } from 'svelte/server'; import Component from 'echo-component-test'; export default props => render(Component, { props }).body;"
          : compile(component as string, { generate: client ? 'client' : 'server' }).js.code,
        loader: 'js', resolveDir
      }));
      build.onLoad({ filter: /\.svelte$/ }, async ({ path }) => ({
        contents: compile(await Bun.file(path).text(), { filename: path, generate: client ? 'client' : 'server' }).js.code,
        loader: 'js', resolveDir: dirname(path)
      }));
      build.onLoad({ filter: /\.svelte\.ts$/ }, async ({ path }) => ({
        contents: compileModule(new Bun.Transpiler({ loader: 'ts' }).transformSync(await Bun.file(path).text()), { filename: path, generate: client ? 'client' : 'server' }).js.code,
        loader: 'ts', resolveDir: dirname(path)
      }));
      build.onLoad({ filter: /\.css$/ }, () => ({ contents: '', loader: 'js' }));
    }}]
  });
  if (!result.success) throw new AggregateError(result.logs, 'Test component compilation failed');
  const directory = await mkdtemp(join(tmpdir(), 'echo-render-'));
  try {
    const path = join(directory, 'render.mjs');
    await Bun.write(path, result.outputs[0]);
    return (await import(path)).default;
  } finally {
    await rm(directory, { recursive: true });
  }
}
