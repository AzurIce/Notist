import { withPackageRegistry, withAccessModel, preloadFontAssets, disableDefaultFontAssets, loadFonts, } from '../options.init.mjs';
import { loadFontSync } from '../init.mjs';
import { MemoryAccessModel } from '../fs/index.mjs';
import { FetchPackageRegistry } from '../fs/package.mjs';
import { randstr } from '../utils.mjs';
import { CompileFormatEnum } from '../compiler.mjs';
const isNode = 
// @ts-ignore
typeof process !== 'undefined' && process.versions != null && process.versions.node != null;
/**
 * Convenient util class for compiling documents, which is a wrapper of the
 * {@link TypstCompiler} and {@link TypstRenderer}.
 *
 * Note: the interface of this class is less stable than {@link TypstCompiler}
 * and {@link TypstRenderer}.
 *
 * @example
 * Use the *global shared* compiler instance:
 *
 * ```typescript
 * import { $typst } from '@myriaddreamin/typst.ts';
 * ```
 *
 * Note: if you want to compile multiple documents, you should create a new
 * instance for each compilation work or maintain the shared state on the
 * utility instance `$typst` carefully, because the compilation process will
 * change the state of that.
 *
 * @example
 * Create an instance of utility:
 *
 * ```typescript
 * const $typst = new TypstSnippet({
 *   // optional renderer instance
 *   renderer: enableRendering ?? (() => {
 *     return createGlobalRenderer(createTypstRenderer,
 *       undefined, initOptions);
 *   }),
 *   compiler() => {
 *     return createGlobalCompiler(createTypstCompiler,
 *       initOptions);
 *   }
 * });
 * ```
 */
export class TypstSnippet {
    /** @internal */
    mainFilePath;
    /** @internal */
    cc;
    /** @internal */
    fr;
    /** @internal */
    ex;
    /**
     * Create a new instance of {@link TypstSnippet}.
     * @param cc the compiler instance, see {@link PromiseJust} and {@link TypstCompiler}.
     * @param ex the renderer instance, see {@link PromiseJust} and {@link TypstRenderer}.
     *
     * @example
     *
     * Passes a global shared compiler instance that get initialized lazily:
     * ```typescript
     * const $typst = new TypstSnippet(() => {
     *  return createGlobalCompiler(createTypstCompiler, initOptions);
     * });
     *
     */
    constructor(options) {
        this.cc = options?.compiler || TypstSnippet.buildLocalCompiler;
        this.fr = options?.fontResolver || TypstSnippet.buildLocalFontResolver;
        this.ex = options?.renderer || TypstSnippet.buildLocalRenderer;
        this.mainFilePath = '/main.typ';
        this.providers = [];
    }
    /**
     * Set lazy initialized compiler instance for the utility instance.
     * @param cc the compiler instance, see {@link PromiseJust} and {@link TypstCompiler}.
     */
    setCompiler(cc) {
        this.cc = cc;
    }
    async getFontResolver() {
        return (typeof this.fr === 'function' ? (this.fr = await this.fr()) : this.fr);
    }
    /**
     * Get an initialized compiler instance from the utility instance.
     */
    async getCompiler() {
        return (typeof this.cc === 'function' ? (this.cc = await this.cc()) : this.cc);
    }
    async getCompilerReset() {
        const compiler = await this.getCompiler();
        await compiler.reset();
        return compiler;
    }
    /**
     * Set lazy initialized renderer instance for the utility instance.
     * @param ex the renderer instance, see {@link PromiseJust} and {@link TypstRenderer}.
     */
    setRenderer(ex) {
        this.ex = ex;
    }
    /**
     * Get an initialized renderer instance from the utility instance.
     */
    async getRenderer() {
        return typeof this.ex === 'function' ? (this.ex = await this.ex()) : this.ex;
    }
    providers;
    /**
     * add providers for bullding the compiler or renderer component.
     */
    use(...providers) {
        if (!this.providers) {
            throw new Error('already prepare uses for instances');
        }
        this.providers.push(...providers);
    }
    /**
     * todo: add docs
     */
    static preloadFontFromUrl(fontUrl) {
        return TypstSnippet.preloadFonts([fontUrl]);
    }
    /**
     * todo: add docs
     */
    static preloadFontData(fontData) {
        return TypstSnippet.preloadFonts([fontData]);
    }
    /**
     * todo: add docs
     */
    static preloadFonts(userFonts) {
        return {
            key: 'access-model',
            forRoles: ['compiler'],
            provides: [loadFonts(userFonts)],
        };
    }
    /**
     * don't load any default font assets.
     * todo: add docs
     */
    static disableDefaultFontAssets() {
        return {
            key: 'access-model',
            forRoles: ['compiler'],
            provides: [disableDefaultFontAssets()],
        };
    }
    /**
     * todo: add docs
     */
    static preloadFontAssets(options) {
        return {
            key: 'access-model',
            forRoles: ['compiler'],
            provides: [preloadFontAssets(options)],
        };
    }
    /**
     * Set accessl model for the compiler instance
     * @example
     *
     * use memory access model
     *
     * ```typescript
     * const m = new MemoryAccessModel();
     * $typst.use(TypstSnippet.withAccessModel(m));
     * ```
     */
    static withAccessModel(accessModel) {
        return {
            key: 'access-model',
            forRoles: ['compiler'],
            provides: [withAccessModel(accessModel)],
        };
    }
    /**
     * Set package registry for the compiler instance
     * @example
     *
     * use a customized package registry
     *
     * ```typescript
     * const n = new NodeFetchPackageRegistry();
     * $typst.use(TypstSnippet.withPackageRegistry(n));
     * ```
     */
    static withPackageRegistry(registry) {
        return {
            key: 'package-registry',
            forRoles: ['compiler'],
            provides: [withPackageRegistry(registry)],
        };
    }
    /**
     * Retrieve an access model to store the data of fetched files.
     * Provide a PackageRegistry instance for the compiler instance.
     *
     * @example
     *
     * use default (memory) access model
     *
     * ```typescript
     * $typst.use(await TypstSnippet.fetchPackageRegistry());
     * ```
     *
     * @example
     *
     * use external access model
     *
     * ```typescript
     * const m = new MemoryAccessModel();
     * $typst.use(TypstSnippet.withAccessModel(m), await TypstSnippet.fetchPackageRegistry(m));
     * ```
     */
    static fetchPackageRegistry(accessModel) {
        const m = accessModel || new MemoryAccessModel();
        const provides = [
            ...(accessModel ? [] : [withAccessModel(m)]),
            withPackageRegistry(new FetchPackageRegistry(m)),
        ];
        return {
            key: 'package-registry$fetch',
            forRoles: ['compiler'],
            provides,
        };
    }
    /**
     * Retrieve a fetcher for fetching package data.
     * Provide a PackageRegistry instance for the compiler instance.
     * @example
     *
     * use a customized fetcher
     *
     * ```typescript
     * import request from 'sync-request-curl';
     * const m = new MemoryAccessModel();
     * $typst.use(TypstSnippet.withAccessModel(m), await TypstSnippet.fetchPackageBy(m, (_, httpUrl) => {
     *   const response = request('GET', this.resolvePath(path), {
     *     insecure: true,
     *   });
     *
     *   if (response.statusCode === 200) {
     *     return response.getBody(undefined);
     *   }
     *   return undefined;
     * }));
     * ```
     */
    static fetchPackageBy(accessModel, fetcher) {
        class HttpPackageRegistry extends FetchPackageRegistry {
            pullPackageData(path) {
                return fetcher(path, this.resolvePath(path));
            }
        }
        return {
            key: 'package-registry$lambda',
            forRoles: ['compiler'],
            provides: [withPackageRegistry(new HttpPackageRegistry(accessModel))],
        };
    }
    /** @internal */
    ccOptions;
    /**
     * Set compiler init options for initializing global instance {@link $typst}.
     * See {@link InitOptions}.
     */
    setCompilerInitOptions(options) {
        this.requireIsUninitialized('compiler', this.cc);
        this.ccOptions = options;
    }
    /** @internal */
    exOptions;
    /**
     * Set renderer init options for initializing global instance {@link $typst}.
     * See {@link InitOptions}.
     */
    setRendererInitOptions(options) {
        this.requireIsUninitialized('renderer', this.ex);
        this.exOptions = options;
    }
    /**
     * Set shared main file path.
     */
    setMainFilePath(path) {
        this.mainFilePath = path;
    }
    /**
     * Get shared main file path.
     */
    getMainFilePath() {
        return this.mainFilePath;
    }
    removeTmp(opts) {
        if (opts.mainFilePath.startsWith('/tmp/')) {
            return this.unmapShadow(opts.mainFilePath);
        }
        return Promise.resolve();
    }
    /**
     * Adds a font to the compiler.
     *
     * @example
     *
     * ```typescript
     * const fonts = await fetch('fontInfo.json').then(res => res.json());
     * $typst.addFonts(fonts.map(font => $typst.loadFont(font.url)));
     * ```
     *
     * @param fontInfos the font infos to add.
     */
    async setFonts(fontInfos) {
        const fb = await this.getFontResolver();
        for (const font of fontInfos) {
            await fb.addLazyFont(font, 'blob' in font ? font.blob : loadFontSync(font), font);
        }
        const compiler = await this.getCompiler();
        await fb.build(async (fonts) => compiler.setFonts(fonts));
    }
    /**
     * Add a source file to the compiler.
     * See {@link TypstCompiler#addSource}.
     */
    async addSource(path, content) {
        (await this.getCompiler()).addSource(path, content);
    }
    /**
     * Reset the shadow files.
     * Note: this function is independent to the {@link reset} function.
     * See {@link TypstCompiler#resetShadow}.
     */
    async resetShadow() {
        (await this.getCompiler()).resetShadow();
    }
    /**
     * Add a shadow file to the compiler.
     * See {@link TypstCompiler#mapShadow}.
     */
    async mapShadow(path, content) {
        (await this.getCompiler()).mapShadow(path, content);
    }
    /**
     * Remove a shadow file from the compiler.
     * See {@link TypstCompiler#unmapShadow}.
     */
    async unmapShadow(path) {
        (await this.getCompiler()).unmapShadow(path);
    }
    /**
     * Compile the document to vector (IR) format.
     * See {@link SweetCompileOptions}.
     */
    async vector(o) {
        const opts = await this.getCompileOptions(o);
        const compiler = await this.getCompilerReset();
        return compiler
            .compile(opts)
            .then(res => res.result)
            .finally(() => this.removeTmp(opts));
    }
    /**
     * Compile the document to PDF format.
     * See {@link SweetCompileOptions}.
     */
    async pdf(o) {
        const opts = await this.getCompileOptions(o);
        opts.format = CompileFormatEnum.pdf;
        const compiler = await this.getCompilerReset();
        return compiler
            .compile(opts)
            .then(res => res.result)
            .finally(() => this.removeTmp(opts));
    }
    /**
     * Compile the document to SVG format.
     * See {@link SweetRenderOptions} and {@link RenderSvgOptions}.
     */
    async svg(o) {
        return this.transientRender(o, (renderer, renderSession) => renderer.renderSvg({
            ...o,
            renderSession,
        }));
    }
    /**
     * Compile the document to canvas operations.
     * See {@link SweetRenderOptions} and {@link RenderToCanvasOptions}.
     */
    async canvas(container, o) {
        return this.transientRender(o, (renderer, renderSession) => renderer.renderToCanvas({
            container,
            ...o,
            renderSession,
        }));
    }
    /**
     * Get semantic tokens for the document.
     */
    async query(o) {
        const opts = await this.getCompileOptions(o);
        const compiler = await this.getCompilerReset();
        return compiler
            .query({
            ...o,
            ...opts,
        })
            .finally(() => this.removeTmp(opts));
    }
    /**
     * Get token legend for semantic tokens.
     */
    async getSemanticTokenLegend() {
        const compiler = await this.getCompilerReset();
        return compiler.getSemanticTokenLegend();
    }
    /**
     * Get semantic tokens for the document.
     * See {@link SweetCompileOptions}.
     * See {@link TypstCompiler#getSemanticTokens}.
     */
    async getSemanticTokens(o) {
        const opts = await this.getCompileOptions(o);
        const compiler = await this.getCompilerReset();
        return compiler
            .getSemanticTokens({
            mainFilePath: opts.mainFilePath,
            resultId: o.resultId,
        })
            .finally(() => this.removeTmp(opts));
    }
    async getCompileOptions(opts) {
        if (opts === undefined) {
            return { mainFilePath: this.mainFilePath, diagnostics: 'none' };
        }
        else if (typeof opts === 'string') {
            throw new Error(`please specify opts as {mainContent: '...'} or {mainFilePath: '...'}`);
        }
        else if ('mainFilePath' in opts) {
            return { ...opts, diagnostics: 'none' };
        }
        else {
            const destFile = `/tmp/${randstr()}.typ`;
            await this.addSource(destFile, opts.mainContent);
            return { mainFilePath: destFile, inputs: opts.inputs, diagnostics: 'none' };
        }
    }
    async getVector(o) {
        if (o && 'vectorData' in o) {
            return o.vectorData;
        }
        const opts = await this.getCompileOptions(o);
        return (await this.getCompiler())
            .compile(opts)
            .then(res => res.result)
            .finally(() => this.removeTmp(opts));
    }
    async transientRender(opts, f) {
        const rr = await this.getRenderer();
        if (!rr) {
            throw new Error('does not provide renderer instance');
        }
        const data = await this.getVector(opts);
        return await rr.runWithSession(async (session) => {
            rr.manipulateData({
                renderSession: session,
                action: 'reset',
                data,
            });
            return f(rr, session);
        });
    }
    prepareUseOnce = undefined;
    async prepareUse() {
        if (this.prepareUseOnce) {
            return this.prepareUseOnce;
        }
        return (this.prepareUseOnce = this.doPrepareUse());
    }
    async doPrepareUse() {
        if (!this.providers) {
            return;
        }
        const providers = await Promise.all(this.providers.map(p => (typeof p === 'function' ? p() : p)));
        this.providers = [];
        if ($typst == this &&
            !providers.some(p => p.key.includes('package-registry') || p.key.includes('access-model'))) {
            // Note: the default fetch backend always adds a withAccessModel(mem)
            if (isNode) {
                const escapeImport = new Function('m', 'return import(m)');
                try {
                    const m = new MemoryAccessModel();
                    const { default: request } = await escapeImport('sync-request');
                    $typst.use(TypstSnippet.withAccessModel(m), TypstSnippet.fetchPackageBy(m, (_, path) => {
                        const response = request('GET', path);
                        if (response.statusCode === 200) {
                            return response.getBody(undefined);
                        }
                        return undefined;
                    }));
                }
                catch (e) { }
            }
            else {
                $typst.use(TypstSnippet.fetchPackageRegistry());
            }
        }
        const providers2 = await Promise.all(this.providers.map(p => (typeof p === 'function' ? p() : p)));
        const ccOptions = (this.ccOptions ||= {});
        const ccBeforeBuild = (ccOptions.beforeBuild ||= []);
        const exOptions = (this.exOptions ||= {});
        const exBeforeBuild = (exOptions.beforeBuild ||= []);
        for (const provider of [...providers, ...providers2]) {
            if (provider.forRoles.includes('compiler')) {
                this.requireIsUninitialized('compiler', this.cc);
                ccBeforeBuild.push(...provider.provides);
            }
            if (provider.forRoles.includes('renderer')) {
                this.requireIsUninitialized('renderer', this.ex);
                exBeforeBuild.push(...provider.provides);
            }
        }
        this.providers = undefined;
    }
    requireIsUninitialized(role, c, e) {
        if (c && typeof c !== 'function') {
            throw new Error(`${role} has been initialized: ${c}`);
        }
    }
    /** @internal */
    static async buildLocalCompiler() {
        const { createTypstCompiler } = (await import(
        // @ts-ignore
        '@myriaddreamin/typst.ts/compiler'));
        await this.prepareUse();
        const compiler = createTypstCompiler();
        await compiler.init(this.ccOptions);
        return compiler;
    }
    /** @internal */
    static async buildLocalFontResolver() {
        const { createTypstFontBuilder } = (await import(
        // @ts-ignore
        '@myriaddreamin/typst.ts/compiler'));
        await this.prepareUse();
        const fonts = createTypstFontBuilder();
        await fonts.init(this.ccOptions);
        return fonts;
    }
    /** @internal */
    static async buildGlobalCompiler() {
        // lazy import compile module
        const { createGlobalCompiler } = (await import(
        // @ts-ignore
        '@myriaddreamin/typst.ts/contrib/global-compiler'));
        const { createTypstCompiler } = (await import(
        // @ts-ignore
        '@myriaddreamin/typst.ts/compiler'));
        await this.prepareUse();
        return createGlobalCompiler(createTypstCompiler, this.ccOptions);
    }
    /** @internal */
    static async buildLocalRenderer() {
        const { createTypstRenderer } = (await import(
        // @ts-ignore
        '@myriaddreamin/typst.ts/renderer'));
        await this.prepareUse();
        const renderer = createTypstRenderer();
        await renderer.init(this.exOptions);
        return renderer;
    }
    /** @internal */
    static async buildGlobalRenderer() {
        // lazy import renderer module
        const { createGlobalRenderer } = (await import(
        // @ts-ignore
        '@myriaddreamin/typst.ts/contrib/global-renderer'));
        const { createTypstRenderer } = (await import(
        // @ts-ignore
        '@myriaddreamin/typst.ts/renderer'));
        await this.prepareUse();
        return createGlobalRenderer(createTypstRenderer, this.exOptions);
    }
}
/**
 * The lazy initialized global shared instance of {@link TypstSnippet}. See
 * {@link TypstSnippet} for more details.
 */
export const $typst = new TypstSnippet({
    compiler: TypstSnippet.buildGlobalCompiler,
    renderer: TypstSnippet.buildGlobalRenderer,
});
//# sourceMappingURL=data:application/json;base64,eyJ2ZXJzaW9uIjozLCJmaWxlIjoic25pcHBldC5tanMiLCJzb3VyY2VSb290IjoiIiwic291cmNlcyI6WyIuLi8uLi8uLi9zcmMvY29udHJpYi9zbmlwcGV0Lm10cyJdLCJuYW1lcyI6W10sIm1hcHBpbmdzIjoiQUFDQSxPQUFPLEVBQ0wsbUJBQW1CLEVBQ25CLGVBQWUsRUFHZixpQkFBaUIsRUFDakIsd0JBQXdCLEVBQ3hCLFNBQVMsR0FFVixNQUFNLHFCQUFxQixDQUFDO0FBQzdCLE9BQU8sRUFBRSxZQUFZLEVBQUUsTUFBTSxhQUFhLENBQUM7QUFHM0MsT0FBTyxFQUFFLGlCQUFpQixFQUE0QixNQUFNLGlCQUFpQixDQUFDO0FBQzlFLE9BQU8sRUFBRSxvQkFBb0IsRUFBRSxNQUFNLG1CQUFtQixDQUFDO0FBT3pELE9BQU8sRUFBRSxPQUFPLEVBQUUsTUFBTSxjQUFjLENBQUM7QUFDdkMsT0FBTyxFQUFFLGlCQUFpQixFQUFFLE1BQU0saUJBQWlCLENBQUM7QUE4RXBELE1BQU0sTUFBTTtBQUNWLGFBQWE7QUFDYixPQUFPLE9BQU8sS0FBSyxXQUFXLElBQUksT0FBTyxDQUFDLFFBQVEsSUFBSSxJQUFJLElBQUksT0FBTyxDQUFDLFFBQVEsQ0FBQyxJQUFJLElBQUksSUFBSSxDQUFDO0FBRTlGOzs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7OztHQW1DRztBQUNILE1BQU0sT0FBTyxZQUFZO0lBQ3ZCLGdCQUFnQjtJQUNSLFlBQVksQ0FBUztJQUM3QixnQkFBZ0I7SUFDUixFQUFFLENBQThCO0lBQ3hDLGdCQUFnQjtJQUNSLEVBQUUsQ0FBaUM7SUFDM0MsZ0JBQWdCO0lBQ1IsRUFBRSxDQUE4QjtJQUV4Qzs7Ozs7Ozs7Ozs7OztPQWFHO0lBQ0gsWUFBWSxPQUlYO1FBQ0MsSUFBSSxDQUFDLEVBQUUsR0FBRyxPQUFPLEVBQUUsUUFBUSxJQUFJLFlBQVksQ0FBQyxrQkFBa0IsQ0FBQztRQUMvRCxJQUFJLENBQUMsRUFBRSxHQUFHLE9BQU8sRUFBRSxZQUFZLElBQUksWUFBWSxDQUFDLHNC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QyxNQUFNLElBQUksQ0FBQyxVQUFVLEVBQUUsQ0FBQztRQUN4QixNQUFNLFFBQVEsR0FBRyxtQkFBbUIsRUFBRSxDQUFDO1FBQ3ZDLE1BQU0sUUFBUSxDQUFDLElBQUksQ0FBQyxJQUFJLENBQUMsU0FBUyxDQUFDLENBQUM7UUFDcEMsT0FBTyxRQUFRLENBQUM7SUFDbEIsQ0FBQztJQUVELGdCQUFnQjtJQUNoQixNQUFNLENBQUMsS0FBSyxDQUFDLG1CQUFtQjtRQUM5Qiw4QkFBOEI7UUFDOUIsTUFBTSxFQUFFLG9CQUFvQixFQUFFLEdBQUcsQ0FBQyxNQUFNLE1BQU07UUFDNUMsYUFBYTtRQUNiLGlEQUFpRCxDQUNsRCxDQUFrRCxDQUFDO1FBQ3BELE1BQU0sRUFBRSxtQkFBbUIsRUFBRSxHQUFHLENBQUMsTUFBTSxNQUFNO1FBQzNDLGFBQWE7UUFDYixrQ0FBa0MsQ0FDbkMsQ0FBNEMsQ0FBQztRQUU5QyxNQUFNLElBQUksQ0FBQyxVQUFVLEVBQUUsQ0FBQztRQUN4QixPQUFPLG9CQUFvQixDQUFDLG1CQUFtQixFQUFFLElBQUksQ0FBQyxTQUFTLENBQUMsQ0FBQztJQUNuRSxDQUFDO0NBQ0Y7QUFFRDs7O0dBR0c7QUFDSCxNQUFNLENBQUMsTUFBTSxNQUFNLEdBQUcsSUFBSSxZQUFZLENBQUM7SUFDckMsUUFBUSxFQUFFLFlBQVksQ0FBQyxtQkFBbUI7SUFDMUMsUUFBUSxFQUFFLFlBQVksQ0FBQyxtQkFBbUI7Q0FDM0MsQ0FBQyxDQUFDIiwic291cmNlc0NvbnRlbnQiOlsiaW1wb3J0IHR5cGUgeyBDb21waWxlT3B0aW9ucywgVHlwc3RDb21waWxlciwgVHlwc3RGb250QnVpbGRlciB9IGZyb20gJy4uL2NvbXBpbGVyLm1qcyc7XG5pbXBvcnQge1xuICB3aXRoUGFja2FnZVJlZ2lzdHJ5LFxuICB3aXRoQWNjZXNzTW9kZWwsXG4gIHR5cGUgQmVmb3JlQnVpbGRGbixcbiAgdHlwZSBJbml0T3B0aW9ucyxcbiAgcHJlbG9hZEZvbnRBc3NldHMsXG4gIGRpc2FibGVEZWZhdWx0Rm9udEFzc2V0cyxcbiAgbG9hZEZvbnRzLFxuICBMb2FkUmVtb3RlQXNzZXRzT3B0aW9ucyxcbn0gZnJvbSAnLi4vb3B0aW9ucy5pbml0Lm1qcyc7XG5pbXBvcnQgeyBsb2FkRm9udFN5bmMgfSBmcm9tICcuLi9pbml0Lm1qcyc7XG5pbXBvcnQgdHlwZSB7IFR5cHN0UmVuZGVyZXIsIFJlbmRlclNlc3Npb24gfSBmcm9tICcuLi9yZW5kZXJlci5tanMnO1xuaW1wb3J0IHR5cGUgeyBSZW5kZXJUb0NhbnZhc09wdGlvbnMsIFJlbmRlclN2Z09wdGlvbnMgfSBmcm9tICcuLi9vcHRpb25zLnJlbmRlci5tanMnO1xuaW1wb3J0IHsgTWVtb3J5QWNjZXNzTW9kZWwsIHR5cGUgV3JpdGFibGVBY2Nlc3NNb2RlbCB9IGZyb20gJy4uL2ZzL2luZGV4Lm1qcyc7XG5pbXBvcnQgeyBGZXRjaFBhY2thZ2VSZWdpc3RyeSB9IGZyb20gJy4uL2ZzL3BhY2thZ2UubWpzJztcbmltcG9ydCB7XG4gIFBhY2thZ2VSZWdpc3RyeSxcbiAgUGFja2FnZVNwZWMsXG4gIFNlbWFudGljVG9rZW5zLFxuICBTZW1hbnRpY1Rva2Vuc0xlZ2VuZCxcbn0gZnJvbSAnLi4vaW50ZXJuYWwudHlwZXMubWpzJztcbmltcG9ydCB7IHJhbmRzdHIgfSBmcm9tICcuLi91dGlscy5tanMnO1xuaW1wb3J0IHsgQ29tcGlsZUZvcm1hdEVudW0gfSBmcm9tICcuLi9jb21waWxlci5tanMnO1xuXG4vKipcbiAqIFNvbWUgZnVuY3Rpb24gdGhhdCByZXR1cm5zIGEgcHJvbWlzZSBvZiB2YWx1ZSBvciBqdXN0IHRoYXQgdmFsdWUuXG4gKi9cbnR5cGUgUHJvbWlzZUp1c3Q8VD4gPSAoKCkgPT4gUHJvbWlzZTxUPikgfCBUO1xuXG5pbnRlcmZhY2UgQ29tcGlsZU9wdGlvbnNDb21tb24ge1xuICAvKipcbiAgICogVGhlIHJvb3Qgb2YgdGhlIG1haW4gZmlsZS5cbiAgICovXG4gIHJvb3Q/OiBzdHJpbmc7XG4gIC8qKlxuICAgKiBBZGRzIGEgc3RyaW5nIGtleS12YWx1ZSBwYWlyIHZpc2libGUgdGhyb3VnaCBgc3lzLmlucHV0c2BcbiAgICpcbiAgICogTm90ZTogcGFzcyBge31gIHRvIGNsZWFyIGBzeXMuaW5wdXRzYFxuICAgKlxuICAgKiBOb3RlOiBXaGVuIHBhc3NpbmcgYHVuZGVmaW5lZGAsIGNvbXBpbGVyIHdpbGwgdXNlIGxhc3Qgc2V0IGBzeXMuaW5wdXRzYC5cbiAgICpcbiAgICogTm90ZTogVGhpcyBtZWFucyB5b3Ugc2hvdWxkIGFsd2F5cyBzcGVjaWZ5IGlucHV0cyB3aGVuIHVzaW5nIGNvbXBpbGVyIGZvciBjb25jdXJyZW50IHRhc2tzLlxuICAgKi9cbiAgaW5wdXRzPzogUmVjb3JkPHN0cmluZywgc3RyaW5nPjtcbn1cblxuLyoqXG4gKiBUaGUgc3dlZXQgb3B0aW9ucyBmb3IgY29tcGlsaW5nIGFuZCByZW5kZXJpbmcgdGhlIGRvY3VtZW50LlxuICovXG5leHBvcnQgdHlwZSBTd2VldENvbXBpbGVPcHRpb25zID0gKFxuICB8IHtcbiAgICAvKipcbiAgICAgKiBUaGUgcGF0aCBvZiB0aGUgbWFpbiBmaWxlLlxuICAgICAqL1xuICAgIG1haW5GaWxlUGF0aDogc3RyaW5nO1xuICB9XG4gIHwge1xuICAgIC8qKlxuICAgICAqIFRoZSBzb3VyY2UgY29udGVudCBvZiB0aGUgbWFpbiBmaWxlLlxuICAgICAqL1xuICAgIG1haW5Db250ZW50OiBzdHJpbmc7XG4gIH1cbikgJlxuICBDb21waWxlT3B0aW9uc0NvbW1vbjtcblxuLyoqXG4gKiBUaGUgc3dlZXQgb3B0aW9ucyBmb3IgY29tcGlsaW5nIGFuZCByZW5kZXJpbmcgdGhlIGRvY3VtZW50LlxuICovXG5leHBvcnQgdHlwZSBTd2VldFJlbmRlck9wdGlvbnMgPVxuICB8IFN3ZWV0Q29tcGlsZU9wdGlvbnNcbiAgfCB7XG4gICAgLyoqXG4gICAgICogVGhlIGFydGlmYWN0IGRhdGEgaW4gdmVjdG9yIGZvcm1hdC5cbiAgICAgKi9cbiAgICB2ZWN0b3JEYXRhOiBVaW50OEFycmF5O1xuICB9O1xuXG5leHBvcnQgdHlwZSBTd2VldExhenlGb250ID0ge1xuICBpbmZvOiBhbnk7XG59ICYgKFxuICAgIHwge1xuICAgICAgYmxvYjogKGluZGV4OiBudW1iZXIpID0+IFVpbnQ4QXJyYXk7XG4gICAgfVxuICAgIHwge1xuICAgICAgdXJsOiBzdHJpbmc7XG4gICAgfVxuICApO1xuXG50eXBlIFJvbGUgPSAnY29tcGlsZXInIHwgJ3JlbmRlcmVyJztcblxuLyoqXG4gKiBUaGUgc3dlZXQgc25pcHBldCBwcm92aWRlciBmb3IgYnVsbGRpbmcgdGhlIGNvbXBpbGVyIG9yIHJlbmRlcmVyIGNvbXBvbmVudC5cbiAqIFNlZSB7QGxpbmsgVHlwc3RTbmlwcGV0I3VzZX0gZm9yIG1vcmUgZGV0YWlscy5cbiAqL1xuZXhwb3J0IGludGVyZmFjZSBUeXBzdFNuaXBwZXRQcm92aWRlciB7XG4gIGtleTogc3RyaW5nO1xuICBmb3JSb2xlczogUm9sZVtdO1xuICBwcm92aWRlczogQmVmb3JlQnVpbGRGbltdO1xufVxuXG5jb25zdCBpc05vZGUgPVxuICAvLyBAdHMtaWdub3JlXG4gIHR5cGVvZiBwcm9jZXNzICE9PSAndW5kZWZpbmVkJyAmJiBwcm9jZXNzLnZlcnNpb25zICE9IG51bGwgJiYgcHJvY2Vzcy52ZXJzaW9ucy5ub2RlICE9IG51bGw7XG5cbi8qKlxuICogQ29udmVuaWVudCB1dGlsIGNsYXNzIGZvciBjb21waWxpbmcgZG9jdW1lbnRzLCB3aGljaCBpcyBhIHdyYXBwZXIgb2YgdGhlXG4gKiB7QGxpbmsgVHlwc3RDb21waWxlcn0gYW5kIHtAbGluayBUeXBzdFJlbmRlcmVyfS5cbiAqXG4gKiBOb3RlOiB0aGUgaW50ZXJmYWNlIG9mIHRoaXMgY2xhc3MgaXMgbGVzcyBzdGFibGUgdGhhbiB7QGxpbmsgVHlwc3RDb21waWxlcn1cbiAqIGFuZCB7QGxpbmsgVHlwc3RSZW5kZXJlcn0uXG4gKlxuICogQGV4YW1wbGVcbiAqIFVzZSB0aGUgKmdsb2JhbCBzaGFyZWQqIGNvbXBpbGVyIGluc3RhbmNlOlxuICpcbiAqIGBgYHR5cGVzY3JpcHRcbiAqIGltcG9ydCB7ICR0eXBzdCB9IGZyb20gJ0BteXJpYWRkcmVhbWluL3R5cHN0LnRzJztcbiAqIGBgYFxuICpcbiAqIE5vdGU6IGlmIHlvdSB3YW50IHRvIGNvbXBpbGUgbXVsdGlwbGUgZG9jdW1lbnRzLCB5b3Ugc2hvdWxkIGNyZWF0ZSBhIG5ld1xuICogaW5zdGFuY2UgZm9yIGVhY2ggY29tcGlsYXRpb24gd29yayBvciBtYWludGFpbiB0aGUgc2hhcmVkIHN0YXRlIG9uIHRoZVxuICogdXRpbGl0eSBpbnN0YW5jZSBgJHR5cHN0YCBjYXJlZnVsbHksIGJlY2F1c2UgdGhlIGNvbXBpbGF0aW9uIHByb2Nlc3Mgd2lsbFxuICogY2hhbmdlIHRoZSBzdGF0ZSBvZiB0aGF0LlxuICpcbiAqIEBleGFtcGxlXG4gKiBDcmVhdGUgYW4gaW5zdGFuY2Ugb2YgdXRpbGl0eTpcbiAqXG4gKiBgYGB0eXBlc2NyaXB0XG4gKiBjb25zdCAkdHlwc3QgPSBuZXcgVHlwc3RTbmlwcGV0KHtcbiAqICAgLy8gb3B0aW9uYWwgcmVuZGVyZXIgaW5zdGFuY2VcbiAqICAgcmVuZGVyZXI6IGVuYWJsZVJlbmRlcmluZyA/PyAoKCkgPT4ge1xuICogICAgIHJldHVybiBjcmVhdGVHbG9iYWxSZW5kZXJlcihjcmVhdGVUeXBzdFJlbmRlcmVyLFxuICogICAgICAgdW5kZWZpbmVkLCBpbml0T3B0aW9ucyk7XG4gKiAgIH0pLFxuICogICBjb21waWxlcigpID0+IHtcbiAqICAgICByZXR1cm4gY3JlYXRlR2xvYmFsQ29tcGlsZXIoY3JlYXRlVHlwc3RDb21waWxlcixcbiAqICAgICAgIGluaXRPcHRpb25zKTtcbiAqICAgfVxuICogfSk7XG4gKiBgYGBcbiAqL1xuZXhwb3J0IGNsYXNzIFR5cHN0U25pcHBldCB7XG4gIC8qKiBAaW50ZXJuYWwgKi9cbiAgcHJpdmF0ZSBtYWluRmlsZVBhdGg6IHN0cmluZztcbiAgLyoqIEBpbnRlcm5hbCAqL1xuICBwcml2YXRlIGNjPzogUHJvbWlzZUp1c3Q8VHlwc3RDb21waWxlcj47XG4gIC8qKiBAaW50ZXJuYWwgKi9cbiAgcHJpdmF0ZSBmcj86IFByb21pc2VKdXN0PFR5cHN0Rm9udEJ1aWxkZXI+O1xuICAvKiogQGludGVybmFsICovXG4gIHByaXZhdGUgZXg/OiBQcm9taXNlSnVzdDxUeXBzdFJlbmRlcmVyPjtcblxuICAvKipcbiAgICogQ3JlYXRlIGEgbmV3IGluc3RhbmNlIG9mIHtAbGluayBUeXBzdFNuaXBwZXR9LlxuICAgKiBAcGFyYW0gY2MgdGhlIGNvbXBpbGVyIGluc3RhbmNlLCBzZWUge0BsaW5rIFByb21pc2VKdXN0fSBhbmQge0BsaW5rIFR5cHN0Q29tcGlsZXJ9LlxuICAgKiBAcGFyYW0gZXggdGhlIHJlbmRlcmVyIGluc3RhbmNlLCBzZWUge0BsaW5rIFByb21pc2VKdXN0fSBhbmQge0BsaW5rIFR5cHN0UmVuZGVyZXJ9LlxuICAgKlxuICAgKiBAZXhhbXBsZVxuICAgKlxuICAgKiBQYXNzZXMgYSBnbG9iYWwgc2hhcmVkIGNvbXBpbGVyIGluc3RhbmNlIHRoYXQgZ2V0IGluaXRpYWxpemVkIGxhemlseTpcbiAgICogYGBgdHlwZXNjcmlwdFxuICAgKiBjb25zdCAkdHlwc3QgPSBuZXcgVHlwc3RTbmlwcGV0KCgpID0+IHtcbiAgICogIHJldHVybiBjcmVhdGVHbG9iYWxDb21waWxlcihjcmVhdGVUeXBzdENvbXBpbGVyLCBpbml0T3B0aW9ucyk7XG4gICAqIH0pO1xuICAgKlxuICAgKi9cbiAgY29uc3RydWN0b3Iob3B0aW9ucz86IHtcbiAgICBjb21waWxlcj86IFByb21pc2VKdXN0PFR5cHN0Q29tcGlsZXI+O1xuICAgIGZvbnRSZXNvbHZlcj86IFByb21pc2VKdXN0PFR5cHN0Rm9udEJ1aWxkZXI+O1xuICAgIHJlbmRlcmVyPzogUHJvbWlzZUp1c3Q8VHlwc3RSZW5kZXJlcj47XG4gIH0pIHtcbiAgICB0aGlzLmNjID0gb3B0aW9ucz8uY29tcGlsZXIgfHwgVHlwc3RTbmlwcGV0LmJ1aWxkTG9jYWxDb21waWxlcjtcbiAgICB0aGlzLmZyID0gb3B0aW9ucz8uZm9udFJlc29sdmVyIHx8IFR5cHN0U25pcHBldC5idWlsZExvY2FsRm9udFJlc29sdmVyO1xuICAgIHRoaXMuZXggPSBvcHRpb25zPy5yZW5kZXJlciB8fCBUeXBzdFNuaXBwZXQuYnVpbGRMb2NhbFJlbmRlcmVyO1xuICAgIHRoaXMubWFpbkZpbGVQYXRoID0gJy9tYWluLnR5cCc7XG4gICAgdGhpcy5wcm92aWRlcnMgPSBbXTtcbiAgfVxuXG4gIC8qKlxuICAgKiBTZXQgbGF6eSBpbml0aWFsaXplZCBjb21waWxlciBpbnN0YW5jZSBmb3IgdGhlIHV0aWxpdHkgaW5zdGFuY2UuXG4gICAqIEBwYXJhbSBjYyB0aGUgY29tcGlsZXIgaW5zdGFuY2UsIHNlZSB7QGxpbmsgUHJvbWlzZUp1c3R9IGFuZCB7QGxpbmsgVHlwc3RDb21waWxlcn0uXG4gICAqL1xuICBzZXRDb21waWxlcihjYzogUHJvbWlzZUp1c3Q8VHlwc3RDb21waWxlcj4pIHtcbiAgICB0aGlzLmNjID0gY2M7XG4gIH1cblxuICBhc3luYyBnZXRGb250UmVzb2x2ZXIoKSB7XG4gICAgcmV0dXJuICh0eXBlb2YgdGhpcy5mciA9PT0gJ2Z1bmN0aW9uJyA/ICh0aGlzLmZyID0gYXdhaXQgdGhpcy5mcigpKSA6IHRoaXMuZnIpITtcbiAgfVxuXG4gIC8qKlxuICAgKiBHZXQgYW4gaW5pdGlhbGl6ZWQgY29tcGlsZXIgaW5zdGFuY2UgZnJvbSB0aGUgdXRpbGl0eSBpbnN0YW5jZS5cbiAgICovXG4gIGFzeW5jIGdldENvbXBpbGVyKCkge1xuICAgIHJldHVybiAodHlwZW9mIHRoaXMuY2MgPT09ICdmdW5jdGlvbicgPyAodGhpcy5jYyA9IGF3YWl0IHRoaXMuY2MoKSkgOiB0aGlzLmNjKSE7XG4gIH1cblxuICBwcml2YXRlIGFzeW5jIGdldENvbXBpbGVyUmVzZXQoKSB7XG4gICAgY29uc3QgY29tcGlsZXIgPSBhd2FpdCB0aGlzLmdldENvbXBpbGVyKCk7XG4gICAgYXdhaXQgY29tcGlsZXIucmVzZXQoKTtcbiAgICByZXR1cm4gY29tcGlsZXI7XG4gIH1cblxuICAvKipcbiAgICogU2V0IGxhenkgaW5pdGlhbGl6ZWQgcmVuZGVyZXIgaW5zdGFuY2UgZm9yIHRoZSB1dGlsaXR5IGluc3RhbmNlLlxuICAgKiBAcGFyYW0gZXggdGhlIHJlbmRlcmVyIGluc3RhbmNlLCBzZWUge0BsaW5rIFByb21pc2VKdXN0fSBhbmQge0BsaW5rIFR5cHN0UmVuZGVyZXJ9LlxuICAgKi9cbiAgc2V0UmVuZGVyZXIoZXg6IFByb21pc2VKdXN0PFR5cHN0UmVuZGVyZXI+KSB7XG4gICAgdGhpcy5leCA9IGV4O1xuICB9XG5cbiAgLyoqXG4gICAqIEdldCBhbiBpbml0aWFsaXplZCByZW5kZXJlciBpbnN0YW5jZSBmcm9tIHRoZSB1dGlsaXR5IGluc3RhbmNlLlxuICAgKi9cbiAgYXN5bmMgZ2V0UmVuZGVyZXIoKTogUHJvbWlzZTxUeXBzdFJlbmRlcmVyPiB7XG4gICAgcmV0dXJuIHR5cGVvZiB0aGlzLmV4ID09PSAnZnVuY3Rpb24nID8gKHRoaXMuZXggPSBhd2FpdCB0aGlzLmV4KCkpIDogdGhpcy5leCE7XG4gIH1cblxuICBwcml2YXRlIHByb3ZpZGVycz86IFByb21pc2VKdXN0PFR5cHN0U25pcHBldFByb3ZpZGVyPltdO1xuICAvKipcbiAgICogYWRkIHByb3ZpZGVycyBmb3IgYnVsbGRpbmcgdGhlIGNvbXBpbGVyIG9yIHJlbmRlcmVyIGNvbXBvbmVudC5cbiAgICovXG4gIHVzZSguLi5wcm92aWRlcnM6IFByb21pc2VKdXN0PFR5cHN0U25pcHBldFByb3ZpZGVyPltdKSB7XG4gICAgaWYgKCF0aGlzLnByb3ZpZGVycykge1xuICAgICAgdGhyb3cgbmV3IEVycm9yKCdhbHJlYWR5IHByZXBhcmUgdXNlcyBmb3IgaW5zdGFuY2VzJyk7XG4gICAgfVxuICAgIHRoaXMucHJvdmlkZXJzLnB1c2goLi4ucHJvdmlkZXJzKTtcbiAgfVxuXG4gIC8qKlxuICAgKiB0b2RvOiBhZGQgZG9jc1xuICAgKi9cbiAgc3RhdGljIHByZWxvYWRGb250RnJvbVVybChmb250VXJsOiBzdHJpbmcpOiBUeXBzdFNuaXBwZXRQcm92aWRlciB7XG4gICAgcmV0dXJuIFR5cHN0U25pcHBldC5wcmVsb2FkRm9udHMoW2ZvbnRVcmxdKTtcbiAgfVxuXG4gIC8qKlxuICAgKiB0b2RvOiBhZGQgZG9jc1xuICAgKi9cbiAgc3RhdGljIHByZWxvYWRGb250RGF0YShmb250RGF0YTogVWludDhBcnJheSk6IFR5cHN0U25pcHBldFByb3ZpZGVyIHtcbiAgICByZXR1cm4gVHlwc3RTbmlwcGV0LnByZWxvYWRGb250cyhbZm9udERhdGFdKTtcbiAgfVxuXG4gIC8qKlxuICAgKiB0b2RvOiBhZGQgZG9jc1xuICAgKi9cbiAgc3RhdGljIHByZWxvYWRGb250cyh1c2VyRm9udHM6IChzdHJpbmcgfCBVaW50OEFycmF5KVtdKTogVHlwc3RTbmlwcGV0UHJvdmlkZXIge1xuICAgIHJldHVybiB7XG4gICAgICBrZXk6ICdhY2Nlc3MtbW9kZWwnLFxuICAgICAgZm9yUm9sZXM6IFsnY29tcGlsZXInXSxcbiAgICAgIHByb3ZpZGVzOiBbbG9hZEZvbnRzKHVzZXJGb250cyldLFxuICAgIH07XG4gIH1cblxuICAvKipcbiAgICogZG9uJ3QgbG9hZCBhbnkgZGVmYXVsdCBmb250IGFzc2V0cy5cbiAgICogdG9kbzogYWRkIGRvY3NcbiAgICovXG4gIHN0YXRpYyBkaXNhYmxlRGVmYXVsdEZvbnRBc3NldHMoKTogVHlwc3RTbmlwcGV0UHJvdmlkZXIge1xuICAgIHJldHVybiB7XG4gICAgICBrZXk6ICdhY2Nlc3MtbW9kZWwnLFxuICAgICAgZm9yUm9sZXM6IFsnY29tcGlsZXInXSxcbiAgICAgIHByb3ZpZGVzOiBbZGlzYWJsZURlZmF1bHRGb250QXNzZXRzKCldLFxuICAgIH07XG4gIH1cblxuICAvKipcbiAgICogdG9kbzogYWRkIGRvY3NcbiAgICovXG4gIHN0YXRpYyBwcmVsb2FkRm9udEFzc2V0cyhvcHRpb25zPzogTG9hZFJlbW90ZUFzc2V0c09wdGlvbnMpOiBUeXBzdFNuaXBwZXRQcm92aWRlciB7XG4gICAgcmV0dXJuIHtcbiAgICAgIGtleTogJ2FjY2Vzcy1tb2RlbCcsXG4gICAgICBmb3JSb2xlczogWydjb21waWxlciddLFxuICAgICAgcHJvdmlkZXM6IFtwcmVsb2FkRm9udEFzc2V0cyhvcHRpb25zKV0sXG4gICAgfTtcbiAgfVxuXG4gIC8qKlxuICAgKiBTZXQgYWNjZXNzbCBtb2RlbCBmb3IgdGhlIGNvbXBpbGVyIGluc3RhbmNlXG4gICAqIEBleGFtcGxlXG4gICAqXG4gICAqIHVzZSBtZW1vcnkgYWNjZXNzIG1vZGVsXG4gICAqXG4gICAqIGBgYHR5cGVzY3JpcHRcbiAgICogY29uc3QgbSA9IG5ldyBNZW1vcnlBY2Nlc3NNb2RlbCgpO1xuICAgKiAkdHlwc3QudXNlKFR5cHN0U25pcHBldC53aXRoQWNjZXNzTW9kZWwobSkpO1xuICAgKiBgYGBcbiAgICovXG4gIHN0YXRpYyB3aXRoQWNjZXNzTW9kZWwoYWNjZXNzTW9kZWw6IFdyaXRhYmxlQWNjZXNzTW9kZWwpOiBUeXBzdFNuaXBwZXRQcm92aWRlciB7XG4gICAgcmV0dXJuIHtcbiAgICAgIGtleTogJ2FjY2Vzcy1tb2RlbCcsXG4gICAgICBmb3JSb2xlczogWydjb21waWxlciddLFxuICAgICAgcHJvdmlkZXM6IFt3aXRoQWNjZXNzTW9kZWwoYWNjZXNzTW9kZWwpXSxcbiAgICB9O1xuICB9XG5cbiAgLyoqXG4gICAqIFNldCBwYWNrYWdlIHJlZ2lzdHJ5IGZvciB0aGUgY29tcGlsZXIgaW5zdGFuY2VcbiAgICogQGV4YW1wbGVcbiAgICpcbiAgICogdXNlIGEgY3VzdG9taXplZCBwYWNrYWdlIHJlZ2lzdHJ5XG4gICAqXG4gICAqIGBgYHR5cGVzY3JpcHRcbiAgICogY29uc3QgbiA9IG5ldyBOb2RlRmV0Y2hQYWNrYWdlUmVnaXN0cnkoKTtcbiAgICogJHR5cHN0LnVzZShUeXBzdFNuaXBwZXQud2l0aFBhY2thZ2VSZWdpc3RyeShuKSk7XG4gICAqIGBgYFxuICAgKi9cbiAgc3RhdGljIHdpdGhQYWNrYWdlUmVnaXN0cnkocmVnaXN0cnk6IFBhY2thZ2VSZWdpc3RyeSk6IFR5cHN0U25pcHBldFByb3ZpZGVyIHtcbiAgICByZXR1cm4ge1xuICAgICAga2V5OiAncGFja2FnZS1yZWdpc3RyeScsXG4gICAgICBmb3JSb2xlczogWydjb21waWxlciddLFxuICAgICAgcHJvdmlkZXM6IFt3aXRoUGFja2FnZVJlZ2lzdHJ5KHJlZ2lzdHJ5KV0sXG4gICAgfTtcbiAgfVxuXG4gIC8qKlxuICAgKiBSZXRyaWV2ZSBhbiBhY2Nlc3MgbW9kZWwgdG8gc3RvcmUgdGhlIGRhdGEgb2YgZmV0Y2hlZCBmaWxlcy5cbiAgICogUHJvdmlkZSBhIFBhY2thZ2VSZWdpc3RyeSBpbnN0YW5jZSBmb3IgdGhlIGNvbXBpbGVyIGluc3RhbmNlLlxuICAgKlxuICAgKiBAZXhhbXBsZVxuICAgKlxuICAgKiB1c2UgZGVmYXVsdCAobWVtb3J5KSBhY2Nlc3MgbW9kZWxcbiAgICpcbiAgICogYGBgdHlwZXNjcmlwdFxuICAgKiAkdHlwc3QudXNlKGF3YWl0IFR5cHN0U25pcHBldC5mZXRjaFBhY2thZ2VSZWdpc3RyeSgpKTtcbiAgICogYGBgXG4gICAqXG4gICAqIEBleGFtcGxlXG4gICAqXG4gICAqIHVzZSBleHRlcm5hbCBhY2Nlc3MgbW9kZWxcbiAgICpcbiAgICogYGBgdHlwZXNjcmlwdFxuICAgKiBjb25zdCBtID0gbmV3IE1lbW9yeUFjY2Vzc01vZGVsKCk7XG4gICAqICR0eXBzdC51c2UoVHlwc3RTbmlwcGV0LndpdGhBY2Nlc3NNb2RlbChtKSwgYXdhaXQgVHlwc3RTbmlwcGV0LmZldGNoUGFja2FnZVJlZ2lzdHJ5KG0pKTtcbiAgICogYGBgXG4gICAqL1xuICBzdGF0aWMgZmV0Y2hQYWNrYWdlUmVnaXN0cnkoYWNjZXNzTW9kZWw/OiBXcml0YWJsZUFjY2Vzc01vZGVsKTogVHlwc3RTbmlwcGV0UHJvdmlkZXIge1xuICAgIGNvbnN0IG0gPSBhY2Nlc3NNb2RlbCB8fCBuZXcgTWVtb3J5QWNjZXNzTW9kZWwoKTtcbiAgICBjb25zdCBwcm92aWRlcyA9IFtcbiAgICAgIC4uLihhY2Nlc3NNb2RlbCA/IFtdIDogW3dpdGhBY2Nlc3NNb2RlbChtKV0pLFxuICAgICAgd2l0aFBhY2thZ2VSZWdpc3RyeShuZXcgRmV0Y2hQYWNrYWdlUmVnaXN0cnkobSkpLFxuICAgIF07XG4gICAgcmV0dXJuIHtcbiAgICAgIGtleTogJ3BhY2thZ2UtcmVnaXN0cnkkZmV0Y2gnLFxuICAgICAgZm9yUm9sZXM6IFsnY29tcGlsZXInXSxcbiAgICAgIHByb3ZpZGVzLFxuICAgIH07XG4gIH1cblxuICAvKipcbiAgICogUmV0cmlldmUgYSBmZXRjaGVyIGZvciBmZXRjaGluZyBwYWNrYWdlIGRhdGEuXG4gICAqIFByb3ZpZGUgYSBQYWNrYWdlUmVnaXN0cnkgaW5zdGFuY2UgZm9yIHRoZSBjb21waWxlciBpbnN0YW5jZS5cbiAgICogQGV4YW1wbGVcbiAgICpcbiAgICogdXNlIGEgY3VzdG9taXplZCBmZXRjaGVyXG4gICAqXG4gICAqIGBgYHR5cGVzY3JpcHRcbiAgICogaW1wb3J0IHJlcXVlc3QgZnJvbSAnc3luYy1yZXF1ZXN0LWN1cmwnO1xuICAgKiBjb25zdCBtID0gbmV3IE1lbW9yeUFjY2Vzc01vZGVsKCk7XG4gICAqICR0eXBzdC51c2UoVHlwc3RTbmlwcGV0LndpdGhBY2Nlc3NNb2RlbChtKSwgYXdhaXQgVHlwc3RTbmlwcGV0LmZldGNoUGFja2FnZUJ5KG0sIChfLCBodHRwVXJsKSA9PiB7XG4gICAqICAgY29uc3QgcmVzcG9uc2UgPSByZXF1ZXN0KCdHRVQnLCB0aGlzLnJlc29sdmVQYXRoKHBhdGgpLCB7XG4gICAqICAgICBpbnNlY3VyZTogdHJ1ZSxcbiAgICogICB9KTtcbiAgICpcbiAgICogICBpZiAocmVzcG9uc2Uuc3RhdHVzQ29kZSA9PT0gMjAwKSB7XG4gICAqICAgICByZXR1cm4gcmVzcG9uc2UuZ2V0Qm9keSh1bmRlZmluZWQpO1xuICAgKiAgIH1cbiAgICogICByZXR1cm4gdW5kZWZpbmVkO1xuICAgKiB9KSk7XG4gICAqIGBgYFxuICAgKi9cbiAgc3RhdGljIGZldGNoUGFja2FnZUJ5KFxuICAgIGFjY2Vzc01vZGVsOiBXcml0YWJsZUFjY2Vzc01vZGVsLFxuICAgIGZldGNoZXI6IChwYXRoOiBQYWNrYWdlU3BlYywgZGVmYXVsdEh0dHBVcmw6IHN0cmluZykgPT4gVWludDhBcnJheSB8IHVuZGVmaW5lZCxcbiAgKTogVHlwc3RTbmlwcGV0UHJvdmlkZXIge1xuICAgIGNsYXNzIEh0dHBQYWNrYWdlUmVnaXN0cnkgZXh0ZW5kcyBGZXRjaFBhY2thZ2VSZWdpc3RyeSB7XG4gICAgICBwdWxsUGFja2FnZURhdGEocGF0aDogUGFja2FnZVNwZWMpOiBVaW50OEFycmF5IHwgdW5kZWZpbmVkIHtcbiAgICAgICAgcmV0dXJuIGZldGNoZXIocGF0aCwgdGhpcy5yZXNvbHZlUGF0aChwYXRoKSk7XG4gICAgICB9XG4gICAgfVxuICAgIHJldHVybiB7XG4gICAgICBrZXk6ICdwYWNrYWdlLXJlZ2lzdHJ5JGxhbWJkYScsXG4gICAgICBmb3JSb2xlczogWydjb21waWxlciddLFxuICAgICAgcHJvdmlkZXM6IFt3aXRoUGFja2FnZVJlZ2lzdHJ5KG5ldyBIdHRwUGFja2FnZVJlZ2lzdHJ5KGFjY2Vzc01vZGVsKSldLFxuICAgIH07XG4gIH1cblxuICAvKiogQGludGVybmFsICovXG4gIGNjT3B0aW9uczogUGFydGlhbDxJbml0T3B0aW9ucz47XG4gIC8qKlxuICAgKiBTZXQgY29tcGlsZXIgaW5pdCBvcHRpb25zIGZvciBpbml0aWFsaXppbmcgZ2xvYmFsIGluc3RhbmNlIHtAbGluayAkdHlwc3R9LlxuICAgKiBTZWUge0BsaW5rIEluaXRPcHRpb25zfS5cbiAgICovXG4gIHNldENvbXBpbGVySW5pdE9wdGlvbnMob3B0aW9uczogUGFydGlhbDxJbml0T3B0aW9ucz4pIHtcbiAgICB0aGlzLnJlcXVpcmVJc1VuaW5pdGlhbGl6ZWQoJ2NvbXBpbGVyJywgdGhpcy5jYyk7XG4gICAgdGhpcy5jY09wdGlvbnMgPSBvcHRpb25zO1xuICB9XG5cbiAgLyoqIEBpbnRlcm5hbCAqL1xuICBleE9wdGlvbnM6IFBhcnRpYWw8SW5pdE9wdGlvbnM+O1xuICAvKipcbiAgICogU2V0IHJlbmRlcmVyIGluaXQgb3B0aW9ucyBmb3IgaW5pdGlhbGl6aW5nIGdsb2JhbCBpbnN0YW5jZSB7QGxpbmsgJHR5cHN0fS5cbiAgICogU2VlIHtAbGluayBJbml0T3B0aW9uc30uXG4gICAqL1xuICBzZXRSZW5kZXJlckluaXRPcHRpb25zKG9wdGlvbnM6IFBhcnRpYWw8SW5pdE9wdGlvbnM+KSB7XG4gICAgdGhpcy5yZXF1aXJlSXNVbmluaXRpYWxpemVkKCdyZW5kZXJlcicsIHRoaXMuZXgpO1xuICAgIHRoaXMuZXhPcHRpb25zID0gb3B0aW9ucztcbiAgfVxuXG4gIC8qKlxuICAgKiBTZXQgc2hhcmVkIG1haW4gZmlsZSBwYXRoLlxuICAgKi9cbiAgc2V0TWFpbkZpbGVQYXRoKHBhdGg6IHN0cmluZykge1xuICAgIHRoaXMubWFpbkZpbGVQYXRoID0gcGF0aDtcbiAgfVxuXG4gIC8qKlxuICAgKiBHZXQgc2hhcmVkIG1haW4gZmlsZSBwYXRoLlxuICAgKi9cbiAgZ2V0TWFpbkZpbGVQYXRoKCkge1xuICAgIHJldHVybiB0aGlzLm1haW5GaWxlUGF0aDtcbiAgfVxuXG4gIHJlbW92ZVRtcChvcHRzOiBDb21waWxlT3B0aW9ucyk6IFByb21pc2U8dm9pZD4ge1xuICAgIGlmIChvcHRzLm1haW5GaWxlUGF0aC5zdGFydHNXaXRoKCcvdG1wLycpKSB7XG4gICAgICByZXR1cm4gdGhpcy51bm1hcFNoYWRvdyhvcHRzLm1haW5GaWxlUGF0aCk7XG4gICAgfVxuXG4gICAgcmV0dXJuIFByb21pc2UucmVzb2x2ZSgpO1xuICB9XG5cbiAgLyoqXG4gICAqIEFkZHMgYSBmb250IHRvIHRoZSBjb21waWxlci5cbiAgICpcbiAgICogQGV4YW1wbGVcbiAgICpcbiAgICogYGBgdHlwZXNjcmlwdFxuICAgKiBjb25zdCBmb250cyA9IGF3YWl0IGZldGNoKCdmb250SW5mby5qc29uJykudGhlbihyZXMgPT4gcmVzLmpzb24oKSk7XG4gICAqICR0eXBzdC5hZGRGb250cyhmb250cy5tYXAoZm9udCA9PiAkdHlwc3QubG9hZEZvbnQoZm9udC51cmwpKSk7XG4gICAqIGBgYFxuICAgKlxuICAgKiBAcGFyYW0gZm9udEluZm9zIHRoZSBmb250IGluZm9zIHRvIGFkZC5cbiAgICovXG4gIGFzeW5jIHNldEZvbnRzKGZvbnRJbmZvczogU3dlZXRMYXp5Rm9udFtdKSB7XG4gICAgY29uc3QgZmIgPSBhd2FpdCB0aGlzLmdldEZvbnRSZXNvbHZlcigpO1xuICAgIGZvciAoY29uc3QgZm9udCBvZiBmb250SW5mb3MpIHtcbiAgICAgIGF3YWl0IGZiLmFkZExhenlGb250KGZvbnQsICdibG9iJyBpbiBmb250ID8gZm9udC5ibG9iIDogbG9hZEZvbnRTeW5jKGZvbnQpLCBmb250KTtcbiAgICB9XG4gICAgY29uc3QgY29tcGlsZXIgPSBhd2FpdCB0aGlzLmdldENvbXBpbGVyKCk7XG4gICAgYXdhaXQgZmIuYnVpbGQoYXN5bmMgZm9udHMgPT4gY29tcGlsZXIuc2V0Rm9udHMoZm9udHMpKTtcbiAgfVxuXG4gIC8qKlxuICAgKiBBZGQgYSBzb3VyY2UgZmlsZSB0byB0aGUgY29tcGlsZXIuXG4gICAqIFNlZSB7QGxpbmsgVHlwc3RDb21waWxlciNhZGRTb3VyY2V9LlxuICAgKi9cbiAgYXN5bmMgYWRkU291cmNlKHBhdGg6IHN0cmluZywgY29udGVudDogc3RyaW5nKSB7XG4gICAgKGF3YWl0IHRoaXMuZ2V0Q29tcGlsZXIoKSkuYWRkU291cmNlKHBhdGgsIGNvbnRlbnQpO1xuICB9XG5cbiAgLyoqXG4gICAqIFJlc2V0IHRoZSBzaGFkb3cgZmlsZXMuXG4gICAqIE5vdGU6IHRoaXMgZnVuY3Rpb24gaXMgaW5kZXBlbmRlbnQgdG8gdGhlIHtAbGluayByZXNldH0gZnVuY3Rpb24uXG4gICAqIFNlZSB7QGxpbmsgVHlwc3RDb21waWxlciNyZXNldFNoYWRvd30uXG4gICAqL1xuICBhc3luYyByZXNldFNoYWRvdygpIHtcbiAgICAoYXdhaXQgdGhpcy5nZXRDb21waWxlcigpKS5yZXNldFNoYWRvdygpO1xuICB9XG5cbiAgLyoqXG4gICAqIEFkZCBhIHNoYWRvdyBmaWxlIHRvIHRoZSBjb21waWxlci5cbiAgICogU2VlIHtAbGluayBUeXBzdENvbXBpbGVyI21hcFNoYWRvd30uXG4gICAqL1xuICBhc3luYyBtYXBTaGFkb3cocGF0aDogc3RyaW5nLCBjb250ZW50OiBVaW50OEFycmF5KSB7XG4gICAgKGF3YWl0IHRoaXMuZ2V0Q29tcGlsZXIoKSkubWFwU2hhZG93KHBhdGgsIGNvbnRlbnQpO1xuICB9XG5cbiAgLyoqXG4gICAqIFJlbW92ZSBhIHNoYWRvdyBmaWxlIGZyb20gdGhlIGNvbXBpbGVyLlxuICAgKiBTZWUge0BsaW5rIFR5cHN0Q29tcGlsZXIjdW5tYXBTaGFkb3d9LlxuICAgKi9cbiAgYXN5bmMgdW5tYXBTaGFkb3cocGF0aDogc3RyaW5nKSB7XG4gICAgKGF3YWl0IHRoaXMuZ2V0Q29tcGlsZXIoKSkudW5tYXBTaGFkb3cocGF0aCk7XG4gIH1cblxuICAvKipcbiAgICogQ29tcGlsZSB0aGUgZG9jdW1lbnQgdG8gdmVjdG9yIChJUikgZm9ybWF0LlxuICAgKiBTZWUge0BsaW5rIFN3ZWV0Q29tcGlsZU9wdGlvbnN9LlxuICAgKi9cbiAgYXN5bmMgdmVjdG9yKG8/OiBTd2VldENvbXBpbGVPcHRpb25zKSB7XG4gICAgY29uc3Qgb3B0cyA9IGF3YWl0IHRoaXMuZ2V0Q29tcGlsZU9wdGlvbnMobyk7XG4gICAgY29uc3QgY29tcGlsZXIgPSBhd2FpdCB0aGlzLmdldENvbXBpbGVyUmVzZXQoKTtcbiAgICByZXR1cm4gY29tcGlsZXJcbiAgICAgIC5jb21waWxlKG9wdHMpXG4gICAgICAudGhlbihyZXMgPT4gcmVzLnJlc3VsdClcbiAgICAgIC5maW5hbGx5KCgpID0+IHRoaXMucmVtb3ZlVG1wKG9wdHMpKTtcbiAgfVxuXG4gIC8qKlxuICAgKiBDb21waWxlIHRoZSBkb2N1bWVudCB0byBQREYgZm9ybWF0LlxuICAgKiBTZWUge0BsaW5rIFN3ZWV0Q29tcGlsZU9wdGlvbnN9LlxuICAgKi9cbiAgYXN5bmMgcGRmKG8/OiBTd2VldENvbXBpbGVPcHRpb25zKSB7XG4gICAgY29uc3Qgb3B0cyA9IGF3YWl0IHRoaXMuZ2V0Q29tcGlsZU9wdGlvbnMobyk7XG4gICAgb3B0cy5mb3JtYXQgPSBDb21waWxlRm9ybWF0RW51bS5wZGY7XG4gICAgY29uc3QgY29tcGlsZXIgPSBhd2FpdCB0aGlzLmdldENvbXBpbGVyUmVzZXQoKTtcbiAgICByZXR1cm4gY29tcGlsZXJcbiAgICAgIC5jb21waWxlKG9wdHMpXG4gICAgICAudGhlbihyZXMgPT4gcmVzLnJlc3VsdClcbiAgICAgIC5maW5hbGx5KCgpID0+IHRoaXMucmVtb3ZlVG1wKG9wdHMpKTtcbiAgfVxuXG4gIC8qKlxuICAgKiBDb21waWxlIHRoZSBkb2N1bWVudCB0byBTVkcgZm9ybWF0LlxuICAgKiBTZWUge0BsaW5rIFN3ZWV0UmVuZGVyT3B0aW9uc30gYW5kIHtAbGluayBSZW5kZXJTdmdPcHRpb25zfS5cbiAgICovXG4gIGFzeW5jIHN2ZyhvPzogU3dlZXRSZW5kZXJPcHRpb25zICYgUmVuZGVyU3ZnT3B0aW9ucykge1xuICAgIHJldHVybiB0aGlzLnRyYW5zaWVudFJlbmRlcihvLCAocmVuZGVyZXIsIHJlbmRlclNlc3Npb24pID0+XG4gICAgICByZW5kZXJlci5yZW5kZXJTdmcoe1xuICAgICAgICAuLi5vLFxuICAgICAgICByZW5kZXJTZXNzaW9uLFxuICAgICAgfSksXG4gICAgKTtcbiAgfVxuXG4gIC8qKlxuICAgKiBDb21waWxlIHRoZSBkb2N1bWVudCB0byBjYW52YXMgb3BlcmF0aW9ucy5cbiAgICogU2VlIHtAbGluayBTd2VldFJlbmRlck9wdGlvbnN9IGFuZCB7QGxpbmsgUmVuZGVyVG9DYW52YXNPcHRpb25zfS5cbiAgICovXG4gIGFzeW5jIGNhbnZhcyhcbiAgICBjb250YWluZXI6IEhUTUxFbGVtZW50LFxuICAgIG8/OiBTd2VldFJlbmRlck9wdGlvbnMgJiBPbWl0PFJlbmRlclRvQ2FudmFzT3B0aW9ucywgJ2NvbnRhaW5lcic+LFxuICApIHtcbiAgICByZXR1cm4gdGhpcy50cmFuc2llbnRSZW5kZXIobywgKHJlbmRlcmVyLCByZW5kZXJTZXNzaW9uKSA9PlxuICAgICAgcmVuZGVyZXIucmVuZGVyVG9DYW52YXMoe1xuICAgICAgICBjb250YWluZXIsXG4gICAgICAgIC4uLm8sXG4gICAgICAgIHJlbmRlclNlc3Npb24sXG4gICAgICB9KSxcbiAgICApO1xuICB9XG5cbiAgLyoqXG4gICAqIEdldCBzZW1hbnRpYyB0b2tlbnMgZm9yIHRoZSBkb2N1bWVudC5cbiAgICovXG4gIGFzeW5jIHF1ZXJ5PFQ+KG86IFN3ZWV0Q29tcGlsZU9wdGlvbnMgJiB7IHNlbGVjdG9yOiBzdHJpbmc7IGZpZWxkPzogc3RyaW5nIH0pOiBQcm9taXNlPFQ+IHtcbiAgICBjb25zdCBvcHRzID0gYXdhaXQgdGhpcy5nZXRDb21waWxlT3B0aW9ucyhvKTtcbiAgICBjb25zdCBjb21waWxlciA9IGF3YWl0IHRoaXMuZ2V0Q29tcGlsZXJSZXNldCgpO1xuICAgIHJldHVybiBjb21waWxlclxuICAgICAgLnF1ZXJ5PFQ+KHtcbiAgICAgICAgLi4ubyxcbiAgICAgICAgLi4ub3B0cyxcbiAgICAgIH0pXG4gICAgICAuZmluYWxseSgoKSA9PiB0aGlzLnJlbW92ZVRtcChvcHRzKSk7XG4gIH1cblxuICAvKipcbiAgICogR2V0IHRva2VuIGxlZ2VuZCBmb3Igc2VtYW50aWMgdG9rZW5zLlxuICAgKi9cbiAgYXN5bmMgZ2V0U2VtYW50aWNUb2tlbkxlZ2VuZCgpOiBQcm9taXNlPFNlbWFudGljVG9rZW5zTGVnZW5kPiB7XG4gICAgY29uc3QgY29tcGlsZXIgPSBhd2FpdCB0aGlzLmdldENvbXBpbGVyUmVzZXQoKTtcbiAgICByZXR1cm4gY29tcGlsZXIuZ2V0U2VtYW50aWNUb2tlbkxlZ2VuZCgpO1xuICB9XG5cbiAgLyoqXG4gICAqIEdldCBzZW1hbnRpYyB0b2tlbnMgZm9yIHRoZSBkb2N1bWVudC5cbiAgICogU2VlIHtAbGluayBTd2VldENvbXBpbGVPcHRpb25zfS5cbiAgICogU2VlIHtAbGluayBUeXBzdENvbXBpbGVyI2dldFNlbWFudGljVG9rZW5zfS5cbiAgICovXG4gIGFzeW5jIGdldFNlbWFudGljVG9rZW5zKG86IFN3ZWV0Q29tcGlsZU9wdGlvbnMgJiB7IHJlc3VsdElkPzogc3RyaW5nIH0pOiBQcm9taXNlPFNlbWFudGljVG9rZW5zPiB7XG4gICAgY29uc3Qgb3B0cyA9IGF3YWl0IHRoaXMuZ2V0Q29tcGlsZU9wdGlvbnMobyk7XG4gICAgY29uc3QgY29tcGlsZXIgPSBhd2FpdCB0aGlzLmdldENvbXBpbGVyUmVzZXQoKTtcbiAgICByZXR1cm4gY29tcGlsZXJcbiAgICAgIC5nZXRTZW1hbnRpY1Rva2Vucyh7XG4gICAgICAgIG1haW5GaWxlUGF0aDogb3B0cy5tYWluRmlsZVBhdGgsXG4gICAgICAgIHJlc3VsdElkOiBvLnJlc3VsdElkLFxuICAgICAgfSlcbiAgICAgIC5maW5hbGx5KCgpID0+IHRoaXMucmVtb3ZlVG1wKG9wdHMpKTtcbiAgfVxuXG4gIHByaXZhdGUgYXN5bmMgZ2V0Q29tcGlsZU9wdGlvbnMoXG4gICAgb3B0cz86IFN3ZWV0Q29tcGlsZU9wdGlvbnMsXG4gICk6IFByb21pc2U8Q29tcGlsZU9wdGlvbnM8Q29tcGlsZUZvcm1hdEVudW0sICdub25lJz4+IHtcbiAgICBpZiAob3B0cyA9PT0gdW5kZWZpbmVkKSB7XG4gICAgICByZXR1cm4geyBtYWluRmlsZVBhdGg6IHRoaXMubWFpbkZpbGVQYXRoLCBkaWFnbm9zdGljczogJ25vbmUnIH07XG4gICAgfSBlbHNlIGlmICh0eXBlb2Ygb3B0cyA9PT0gJ3N0cmluZycpIHtcbiAgICAgIHRocm93IG5ldyBFcnJvcihgcGxlYXNlIHNwZWNpZnkgb3B0cyBhcyB7bWFpbkNvbnRlbnQ6ICcuLi4nfSBvciB7bWFpbkZpbGVQYXRoOiAnLi4uJ31gKTtcbiAgICB9IGVsc2UgaWYgKCdtYWluRmlsZVBhdGgnIGluIG9wdHMpIHtcbiAgICAgIHJldHVybiB7IC4uLm9wdHMsIGRpYWdub3N0aWNzOiAnbm9uZScgfTtcbiAgICB9IGVsc2Uge1xuICAgICAgY29uc3QgZGVzdEZpbGUgPSBgL3RtcC8ke3JhbmRzdHIoKX0udHlwYDtcbiAgICAgIGF3YWl0IHRoaXMuYWRkU291cmNlKGRlc3RGaWxlLCBvcHRzLm1haW5Db250ZW50KTtcbiAgICAgIHJldHVybiB7IG1haW5GaWxlUGF0aDogZGVzdEZpbGUsIGlucHV0czogb3B0cy5pbnB1dHMsIGRpYWdub3N0aWNzOiAnbm9uZScgfTtcbiAgICB9XG4gIH1cblxuICBwcml2YXRlIGFzeW5jIGdldFZlY3RvcihvPzogU3dlZXRSZW5kZXJPcHRpb25zKTogUHJvbWlzZTxVaW50OEFycmF5PiB7XG4gICAgaWYgKG8gJiYgJ3ZlY3RvckRhdGEnIGluIG8pIHtcbiAgICAgIHJldHVybiBvLnZlY3RvckRhdGE7XG4gICAgfVxuXG4gICAgY29uc3Qgb3B0cyA9IGF3YWl0IHRoaXMuZ2V0Q29tcGlsZU9wdGlvbnMobyk7XG4gICAgcmV0dXJuIChhd2FpdCB0aGlzLmdldENvbXBpbGVyKCkpXG4gICAgICAuY29tcGlsZShvcHRzKVxuICAgICAgLnRoZW4ocmVzID0+IHJlcy5yZXN1bHQhKVxuICAgICAgLmZpbmFsbHkoKCkgPT4gdGhpcy5yZW1vdmVUbXAob3B0cykpO1xuICB9XG5cbiAgcHJpdmF0ZSBhc3luYyB0cmFuc2llbnRSZW5kZXI8VD4oXG4gICAgb3B0czogU3dlZXRSZW5kZXJPcHRpb25zIHwgdW5kZWZpbmVkLFxuICAgIGY6IChycjogVHlwc3RSZW5kZXJlciwgc2Vzc2lvbjogUmVuZGVyU2Vzc2lvbikgPT4gVCxcbiAgKTogUHJvbWlzZTxUPiB7XG4gICAgY29uc3QgcnIgPSBhd2FpdCB0aGlzLmdldFJlbmRlcmVyKCk7XG4gICAgaWYgKCFycikge1xuICAgICAgdGhyb3cgbmV3IEVycm9yKCdkb2VzIG5vdCBwcm92aWRlIHJlbmRlcmVyIGluc3RhbmNlJyk7XG4gICAgfVxuICAgIGNvbnN0IGRhdGEgPSBhd2FpdCB0aGlzLmdldFZlY3RvcihvcHRzKTtcbiAgICByZXR1cm4gYXdhaXQgcnIucnVuV2l0aFNlc3Npb24oYXN5bmMgc2Vzc2lvbiA9PiB7XG4gICAgICByci5tYW5pcHVsYXRlRGF0YSh7XG4gICAgICAgIHJlbmRlclNlc3Npb246IHNlc3Npb24sXG4gICAgICAgIGFjdGlvbjogJ3Jlc2V0JyxcbiAgICAgICAgZGF0YSxcbiAgICAgIH0pO1xuICAgICAgcmV0dXJuIGYocnIsIHNlc3Npb24pO1xuICAgIH0pO1xuICB9XG5cbiAgcHJlcGFyZVVzZU9uY2U6IFByb21pc2U8dm9pZD4gfCB1bmRlZmluZWQgPSB1bmRlZmluZWQ7XG4gIHByaXZhdGUgYXN5bmMgcHJlcGFyZVVzZSgpIHtcbiAgICBpZiAodGhpcy5wcmVwYXJlVXNlT25jZSkge1xuICAgICAgcmV0dXJuIHRoaXMucHJlcGFyZVVzZU9uY2U7XG4gICAgfVxuICAgIHJldHVybiAodGhpcy5wcmVwYXJlVXNlT25jZSA9IHRoaXMuZG9QcmVwYXJlVXNlKCkpO1xuICB9XG5cbiAgcHJpdmF0ZSBhc3luYyBkb1ByZXBhcmVVc2UoKSB7XG4gICAgaWYgKCF0aGlzLnByb3ZpZGVycykge1xuICAgICAgcmV0dXJuO1xuICAgIH1cblxuICAgIGNvbnN0IHByb3ZpZGVycyA9IGF3YWl0IFByb21pc2UuYWxsKFxuICAgICAgdGhpcy5wcm92aWRlcnMubWFwKHAgPT4gKHR5cGVvZiBwID09PSAnZnVuY3Rpb24nID8gcCgpIDogcCkpLFxuICAgICk7XG4gICAgdGhpcy5wcm92aWRlcnMgPSBbXTtcblxuICAgIGlmIChcbiAgICAgICR0eXBzdCA9PSB0aGlzICYmXG4gICAgICAhcHJvdmlkZXJzLnNvbWUocCA9PiBwLmtleS5pbmNsdWRlcygncGFja2FnZS1yZWdpc3RyeScpIHx8IHAua2V5LmluY2x1ZGVzKCdhY2Nlc3MtbW9kZWwnKSlcbiAgICApIHtcbiAgICAgIC8vIE5vdGU6IHRoZSBkZWZhdWx0IGZldGNoIGJhY2tlbmQgYWx3YXlzIGFkZHMgYSB3aXRoQWNjZXNzTW9kZWwobWVtKVxuICAgICAgaWYgKGlzTm9kZSkge1xuICAgICAgICBjb25zdCBlc2NhcGVJbXBvcnQgPSBuZXcgRnVuY3Rpb24oJ20nLCAncmV0dXJuIGltcG9ydChtKScpO1xuICAgICAgICB0cnkge1xuICAgICAgICAgIGNvbnN0IG0gPSBuZXcgTWVtb3J5QWNjZXNzTW9kZWwoKTtcbiAgICAgICAgICBjb25zdCB7IGRlZmF1bHQ6IHJlcXVlc3QgfSA9IGF3YWl0IGVzY2FwZUltcG9ydCgnc3luYy1yZXF1ZXN0Jyk7XG5cbiAgICAgICAgICAkdHlwc3QudXNlKFxuICAgICAgICAgICAgVHlwc3RTbmlwcGV0LndpdGhBY2Nlc3NNb2RlbChtKSxcbiAgICAgICAgICAgIFR5cHN0U25pcHBldC5mZXRjaFBhY2thZ2VCeShtLCAoXzogdW5rbm93biwgcGF0aDogc3RyaW5nKSA9PiB7XG4gICAgICAgICAgICAgIGNvbnN0IHJlc3BvbnNlID0gcmVxdWVzdCgnR0VUJywgcGF0aCk7XG5cbiAgICAgICAgICAgICAgaWYgKHJlc3BvbnNlLnN0YXR1c0NvZGUgPT09IDIwMCkge1xuICAgICAgICAgICAgICAgIHJldHVybiByZXNwb25zZS5nZXRCb2R5KHVuZGVmaW5lZCk7XG4gICAgICAgICAgICAgIH1cbiAgICAgICAgICAgICAgcmV0dXJuIHVuZGVmaW5lZDtcbiAgICAgICAgICAgIH0pLFxuICAgICAgICAgICk7XG4gICAgICAgIH0gY2F0Y2ggKGUpIHsgfVxuICAgICAgfSBlbHNlIHtcbiAgICAgICAgJHR5cHN0LnVzZShUeXBzdFNuaXBwZXQuZmV0Y2hQYWNrYWdlUmVnaXN0cnkoKSk7XG4gICAgICB9XG4gICAgfVxuXG4gICAgY29uc3QgcHJvdmlkZXJzMiA9IGF3YWl0IFByb21pc2UuYWxsKFxuICAgICAgdGhpcy5wcm92aWRlcnMubWFwKHAgPT4gKHR5cGVvZiBwID09PSAnZnVuY3Rpb24nID8gcCgpIDogcCkpLFxuICAgICk7XG5cbiAgICBjb25zdCBjY09wdGlvbnMgPSAodGhpcy5jY09wdGlvbnMgfHw9IHt9KTtcbiAgICBjb25zdCBjY0JlZm9yZUJ1aWxkID0gKGNjT3B0aW9ucy5iZWZvcmVCdWlsZCB8fD0gW10pO1xuXG4gICAgY29uc3QgZXhPcHRpb25zID0gKHRoaXMuZXhPcHRpb25zIHx8PSB7fSk7XG4gICAgY29uc3QgZXhCZWZvcmVCdWlsZCA9IChleE9wdGlvbnMuYmVmb3JlQnVpbGQgfHw9IFtdKTtcblxuICAgIGZvciAoY29uc3QgcHJvdmlkZXIgb2YgWy4uLnByb3ZpZGVycywgLi4ucHJvdmlkZXJzMl0pIHtcbiAgICAgIGlmIChwcm92aWRlci5mb3JSb2xlcy5pbmNsdWRlcygnY29tcGlsZXInKSkge1xuICAgICAgICB0aGlzLnJlcXVpcmVJc1VuaW5pdGlhbGl6ZWQoJ2NvbXBpbGVyJywgdGhpcy5jYyk7XG4gICAgICAgIGNjQmVmb3JlQnVpbGQucHVzaCguLi5wcm92aWRlci5wcm92aWRlcyk7XG4gICAgICB9XG4gICAgICBpZiAocHJvdmlkZXIuZm9yUm9sZXMuaW5jbHVkZXMoJ3JlbmRlcmVyJykpIHtcbiAgICAgICAgdGhpcy5yZXF1aXJlSXNVbmluaXRpYWxpemVkKCdyZW5kZXJlcicsIHRoaXMuZXgpO1xuICAgICAgICBleEJlZm9yZUJ1aWxkLnB1c2goLi4ucHJvdmlkZXIucHJvdmlkZXMpO1xuICAgICAgfVxuICAgIH1cbiAgICB0aGlzLnByb3ZpZGVycyA9IHVuZGVmaW5lZDtcbiAgfVxuXG4gIHByaXZhdGUgcmVxdWlyZUlzVW5pbml0aWFsaXplZDxUPihyb2xlOiBzdHJpbmcsIGM6IFByb21pc2VKdXN0PFQ+LCBlPzogUHJvbWlzZUp1c3Q8VD4pIHtcbiAgICBpZiAoYyAmJiB0eXBlb2YgYyAhPT0gJ2Z1bmN0aW9uJykge1xuICAgICAgdGhyb3cgbmV3IEVycm9yKGAke3JvbGV9IGhhcyBiZWVuIGluaXRpYWxpemVkOiAke2N9YCk7XG4gICAgfVxuICB9XG5cbiAgLyoqIEBpbnRlcm5hbCAqL1xuICBzdGF0aWMgYXN5bmMgYnVpbGRMb2NhbENvbXBpbGVyKHRoaXM6IFR5cHN0U25pcHBldCkge1xuICAgIGNvbnN0IHsgY3JlYXRlVHlwc3RDb21waWxlciB9ID0gKGF3YWl0IGltcG9ydChcbiAgICAgIC8vIEB0cy1pZ25vcmVcbiAgICAgICdAbXlyaWFkZHJlYW1pbi90eXBzdC50cy9jb21waWxlcidcbiAgICApKSBhcyBhbnkgYXMgdHlwZW9mIGltcG9ydCgnLi4vY29tcGlsZXIubWpzJyk7XG5cbiAgICBhd2FpdCB0aGlzLnByZXBhcmVVc2UoKTtcbiAgICBjb25zdCBjb21waWxlciA9IGNyZWF0ZVR5cHN0Q29tcGlsZXIoKTtcbiAgICBhd2FpdCBjb21waWxlci5pbml0KHRoaXMuY2NPcHRpb25zKTtcbiAgICByZXR1cm4gY29tcGlsZXI7XG4gIH1cblxuICAvKiogQGludGVybmFsICovXG4gIHN0YXRpYyBhc3luYyBidWlsZExvY2FsRm9udFJlc29sdmVyKHRoaXM6IFR5cHN0U25pcHBldCkge1xuICAgIGNvbnN0IHsgY3JlYXRlVHlwc3RGb250QnVpbGRlciB9ID0gKGF3YWl0IGltcG9ydChcbiAgICAgIC8vIEB0cy1pZ25vcmVcbiAgICAgICdAbXlyaWFkZHJlYW1pbi90eXBzdC50cy9jb21waWxlcidcbiAgICApKSBhcyBhbnkgYXMgdHlwZW9mIGltcG9ydCgnLi4vY29tcGlsZXIubWpzJyk7XG5cbiAgICBhd2FpdCB0aGlzLnByZXBhcmVVc2UoKTtcbiAgICBjb25zdCBmb250cyA9IGNyZWF0ZVR5cHN0Rm9udEJ1aWxkZXIoKTtcbiAgICBhd2FpdCBmb250cy5pbml0KHRoaXMuY2NPcHRpb25zKTtcbiAgICByZXR1cm4gZm9udHM7XG4gIH1cblxuICAvKiogQGludGVybmFsICovXG4gIHN0YXRpYyBhc3luYyBidWlsZEdsb2JhbENvbXBpbGVyKHRoaXM6IFR5cHN0U25pcHBldCkge1xuICAgIC8vIGxhenkgaW1wb3J0IGNvbXBpbGUgbW9kdWxlXG4gICAgY29uc3QgeyBjcmVhdGVHbG9iYWxDb21waWxlciB9ID0gKGF3YWl0IGltcG9ydChcbiAgICAgIC8vIEB0cy1pZ25vcmVcbiAgICAgICdAbXlyaWFkZHJlYW1pbi90eXBzdC50cy9jb250cmliL2dsb2JhbC1jb21waWxlcidcbiAgICApKSBhcyBhbnkgYXMgdHlwZW9mIGltcG9ydCgnLi9nbG9iYWwtY29tcGlsZXIubWpzJyk7XG4gICAgY29uc3QgeyBjcmVhdGVUeXBzdENvbXBpbGVyIH0gPSAoYXdhaXQgaW1wb3J0KFxuICAgICAgLy8gQHRzLWlnbm9yZVxuICAgICAgJ0BteXJpYWRkcmVhbWluL3R5cHN0LnRzL2NvbXBpbGVyJ1xuICAgICkpIGFzIGFueSBhcyB0eXBlb2YgaW1wb3J0KCcuLi9jb21waWxlci5tanMnKTtcblxuICAgIGF3YWl0IHRoaXMucHJlcGFyZVVzZSgpO1xuICAgIHJldHVybiBjcmVhdGVHbG9iYWxDb21waWxlcihjcmVhdGVUeXBzdENvbXBpbGVyLCB0aGlzLmNjT3B0aW9ucyk7XG4gIH1cblxuICAvKiogQGludGVybmFsICovXG4gIHN0YXRpYyBhc3luYyBidWlsZExvY2FsUmVuZGVyZXIodGhpczogVHlwc3RTbmlwcGV0KSB7XG4gICAgY29uc3QgeyBjcmVhdGVUeXBzdFJlbmRlcmVyIH0gPSAoYXdhaXQgaW1wb3J0KFxuICAgICAgLy8gQHRzLWlnbm9yZVxuICAgICAgJ0BteXJpYWRkcmVhbWluL3R5cHN0LnRzL3JlbmRlcmVyJ1xuICAgICkpIGFzIGFueSBhcyB0eXBlb2YgaW1wb3J0KCcuLi9yZW5kZXJlci5tanMnKTtcblxuICAgIGF3YWl0IHRoaXMucHJlcGFyZVVzZSgpO1xuICAgIGNvbnN0IHJlbmRlcmVyID0gY3JlYXRlVHlwc3RSZW5kZXJlcigpO1xuICAgIGF3YWl0IHJlbmRlcmVyLmluaXQodGhpcy5leE9wdGlvbnMpO1xuICAgIHJldHVybiByZW5kZXJlcjtcbiAgfVxuXG4gIC8qKiBAaW50ZXJuYWwgKi9cbiAgc3RhdGljIGFzeW5jIGJ1aWxkR2xvYmFsUmVuZGVyZXIodGhpczogVHlwc3RTbmlwcGV0KSB7XG4gICAgLy8gbGF6eSBpbXBvcnQgcmVuZGVyZXIgbW9kdWxlXG4gICAgY29uc3QgeyBjcmVhdGVHbG9iYWxSZW5kZXJlciB9ID0gKGF3YWl0IGltcG9ydChcbiAgICAgIC8vIEB0cy1pZ25vcmVcbiAgICAgICdAbXlyaWFkZHJlYW1pbi90eXBzdC50cy9jb250cmliL2dsb2JhbC1yZW5kZXJlcidcbiAgICApKSBhcyBhbnkgYXMgdHlwZW9mIGltcG9ydCgnLi9nbG9iYWwtcmVuZGVyZXIubWpzJyk7XG4gICAgY29uc3QgeyBjcmVhdGVUeXBzdFJlbmRlcmVyIH0gPSAoYXdhaXQgaW1wb3J0KFxuICAgICAgLy8gQHRzLWlnbm9yZVxuICAgICAgJ0BteXJpYWRkcmVhbWluL3R5cHN0LnRzL3JlbmRlcmVyJ1xuICAgICkpIGFzIGFueSBhcyB0eXBlb2YgaW1wb3J0KCcuLi9yZW5kZXJlci5tanMnKTtcblxuICAgIGF3YWl0IHRoaXMucHJlcGFyZVVzZSgpO1xuICAgIHJldHVybiBjcmVhdGVHbG9iYWxSZW5kZXJlcihjcmVhdGVUeXBzdFJlbmRlcmVyLCB0aGlzLmV4T3B0aW9ucyk7XG4gIH1cbn1cblxuLyoqXG4gKiBUaGUgbGF6eSBpbml0aWFsaXplZCBnbG9iYWwgc2hhcmVkIGluc3RhbmNlIG9mIHtAbGluayBUeXBzdFNuaXBwZXR9LiBTZWVcbiAqIHtAbGluayBUeXBzdFNuaXBwZXR9IGZvciBtb3JlIGRldGFpbHMuXG4gKi9cbmV4cG9ydCBjb25zdCAkdHlwc3QgPSBuZXcgVHlwc3RTbmlwcGV0KHtcbiAgY29tcGlsZXI6IFR5cHN0U25pcHBldC5idWlsZEdsb2JhbENvbXBpbGVyLFxuICByZW5kZXJlcjogVHlwc3RTbmlwcGV0LmJ1aWxkR2xvYmFsUmVuZGVyZXIsXG59KTtcbiJdfQ==