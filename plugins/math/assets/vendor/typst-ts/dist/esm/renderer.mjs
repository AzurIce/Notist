import { TypstDefaultParams, kObject } from './internal.types.mjs';
import { RenderView } from './render/canvas/view.mjs';
import { LazyWasmModule } from './wasm.mjs';
import { buildComponent } from './init.mjs';
import { TypstDomDocument } from './dom.mjs';
/**
 * The session of a Typst document.
 * @property {string} backgroundColor - The background color of the Typst
 * document.
 * @property {number} pixelPerPt - The pixel per point scale up the image.
 *
 */
export class RenderSession {
    plugin;
    /**
     * @internal
     */
    [kObject];
    /**
     * @internal
     */
    constructor(
    /**
     * @internal
     */
    plugin, o) {
        this.plugin = plugin;
        this[kObject] = o;
    }
    /**
     * @deprecated set in {@link RenderToCanvasOptions} instead
     *
     * Set the background color of the Typst document.
     * @param {string} t - The background color in format of `^#?[0-9a-f]{6}$`
     *
     * Note: Default to `#ffffff`.
     *
     * Note: Only available in canvas rendering mode.
     */
    set backgroundColor(t) {
        if (t !== undefined) {
            this[kObject].background_color = t;
        }
    }
    /**
     * Get the background color of the Typst document.
     *
     * Note: Default to `#ffffff`.
     *
     * Note: Only available in canvas rendering mode.
     */
    get backgroundColor() {
        return this[kObject].background_color;
    }
    /**
     * Set the pixel per point scale up the canvas panel.
     *
     * Note: Default to `3`.
     *
     * Note: Only available in canvas rendering mode.
     */
    set pixelPerPt(t) {
        if (t !== undefined) {
            this[kObject].pixel_per_pt = t;
        }
    }
    /**
     * @deprecated set in {@link RenderToCanvasOptions} instead
     *
     * Get the pixel per point scale up the canvas panel.
     *
     * Note: Default to `3`.
     *
     * Note: Only available in canvas rendering mode.
     */
    get pixelPerPt() {
        return this[kObject].pixel_per_pt;
    }
    /**
     * Reset state
     */
    reset() {
        this.plugin.resetSession(this);
    }
    /**
     * @deprecated
     * use {@link docWidth} instead
     */
    get doc_width() {
        return this[kObject].doc_width;
    }
    get docWidth() {
        return this[kObject].doc_width;
    }
    /**
     * @deprecated
     * use {@link docHeight} instead
     */
    get doc_height() {
        return this[kObject].doc_height;
    }
    get docHeight() {
        return this[kObject].doc_height;
    }
    retrievePagesInfo() {
        const pages_info = this[kObject].pages_info;
        const pageInfos = [];
        const pageCount = pages_info.page_count;
        for (let i = 0; i < pageCount; i++) {
            const pageAst = pages_info.page(i);
            pageInfos.push({
                pageOffset: pageAst.page_off,
                width: pageAst.width_pt,
                height: pageAst.height_pt,
            });
        }
        return pageInfos;
    }
    getSourceLoc(path) {
        return this[kObject].source_span(path);
    }
    /**
     * See {@link TypstRenderer#renderSvg} for more details.
     */
    renderSvg(options) {
        return this.plugin.renderSvg({
            renderSession: this,
            ...options,
        });
    }
    /**
     * See {@link TypstRenderer#renderToSvg} for more details.
     */
    renderToSvg(options) {
        return this.plugin.renderToSvg({
            renderSession: this,
            ...options,
        });
    }
    /**
     * See {@link TypstRenderer#renderCanvas} for more details.
     */
    renderCanvas(options) {
        return this.plugin.renderCanvas({
            renderSession: this,
            ...options,
        });
    }
    /**
     * See {@link TypstRenderer#manipulateData} for more details.
     */
    manipulateData(opts) {
        this.plugin.manipulateData({
            renderSession: this,
            ...opts,
        });
    }
    /**
     * See {@link TypstRenderer#renderSvgDiff} for more details.
     */
    renderSvgDiff(opts) {
        return this.plugin.renderSvgDiff({
            renderSession: this,
            ...opts,
        });
    }
    /**
     * @deprecated
     * use {@link getSourceLoc} instead
     */
    get_source_loc(path) {
        return this[kObject].source_span(path);
    }
    /**
     * @deprecated
     * use {@link renderSvgDiff} instead
     */
    render_in_window(rect_lo_x, rect_lo_y, rect_hi_x, rect_hi_y) {
        return this[kObject].render_in_window(rect_lo_x, rect_lo_y, rect_hi_x, rect_hi_y);
    }
    /**
     * @deprecated
     * use {@link manipulateData} instead
     */
    merge_delta(data) {
        this.plugin.manipulateData({
            renderSession: this,
            action: 'merge',
            data,
        });
    }
}
/**
 * @internal
 */
var ManageStatus;
(function (ManageStatus) {
    ManageStatus[ManageStatus["Delete"] = 0] = "Delete";
    ManageStatus[ManageStatus["New"] = 1] = "New";
    ManageStatus[ManageStatus["Update"] = 2] = "Update";
})(ManageStatus || (ManageStatus = {}));
export class TypstWorker {
    plugin;
    /**
     * @internal
     */
    [kObject];
    /**
     * @internal
     */
    constructor(
    /**
     * @internal
     */
    plugin, o) {
        this.plugin = plugin;
        this[kObject] = o;
    }
    /**
     * See {@link TypstRenderer#manipulateData} for more details.
     */
    manipulateData(action, data) {
        return this[kObject].manipulate_data(action, data);
    }
    /**
     * @internal
     */
    managedCanvasElemList = new Map();
    /**
     * @internal
     */
    canvasCounter = Math.random();
    /**
     * You must submit all canvas in pages to ensure synchronization with the background worker
     *
     * See {@link TypstRenderer#renderCanvas} for more details.
     */
    renderCanvas(canvasElemList) {
        const m = this.managedCanvasElemList;
        for (const [_, elem] of m) {
            elem[0] = ManageStatus.Delete;
        }
        for (const elem of canvasElemList) {
            const canvas = elem.canvas;
            let elemId = canvas.dataset.manageId;
            let action = ManageStatus.Update;
            if (!elemId) {
                elemId = this.canvasCounter.toFixed(5);
                this.canvasCounter += 1;
                canvas.dataset.manageId = elemId;
                action = ManageStatus.New;
            }
            let prev = m.get(elemId);
            if (prev && prev[0] !== ManageStatus.Delete) {
                throw new Error('cannot update a canvas for two times in batch');
            }
            m.set(elemId, [action, { ...elem }]);
        }
        const entries = Array.from(m.entries());
        const actions = new Uint8Array(entries.length);
        const elements = new Array(entries.length);
        const options = entries.map(([key, [action, elem]], index) => {
            if (!action) {
                m.delete(key);
            }
            actions[index] = action;
            elements[index] = elem.canvas;
            return this.plugin.canvasOptionsToRust(elem);
        });
        return this[kObject].render_canvas(actions, elements, options);
    }
    async retrievePagesInfo() {
        const pages_info = await this[kObject].get_pages_info();
        console.log(pages_info);
        const pageInfos = [];
        const pageCount = pages_info.page_count;
        for (let i = 0; i < pageCount; i++) {
            const pageAst = pages_info.page(i);
            pageInfos.push({
                pageOffset: pageAst.page_off,
                width: pageAst.width_pt,
                height: pageAst.height_pt,
            });
        }
        return pageInfos;
    }
}
const gRendererModule = (module) => new LazyWasmModule(async (bin) => {
    return await module.default(bin);
});
/**
 * create a Typst renderer.
 * @returns {TypstRenderer} - The Typst renderer.
 * @example
 * ```typescript
 * import { createTypstRenderer } from 'typst';
 * const renderer = createTypstRenderer();
 * await renderer.init();
 * await renderer.render({
 *   container: document.getElementById('container'),
 *   artifactContent: '{ ... }',
 * });
 * ```
 */
export function createTypstRenderer() {
    return new TypstRendererDriver();
}
export async function rendererBuildInfo() {
    const renderModule = await import('@myriaddreamin/typst-ts-renderer');
    return renderModule.renderer_build_info();
}
let warnOnceCanvasSet = true;
/** @internal */
export class TypstRendererDriver {
    renderer;
    rendererJs;
    constructor() { }
    async init(options) {
        this.rendererJs = await (options?.getWrapper?.() || import('@myriaddreamin/typst-ts-renderer'));
        const TypstRendererBuilder = this.rendererJs.TypstRendererBuilder;
        this.renderer = await buildComponent(options, gRendererModule(this.rendererJs), TypstRendererBuilder, {});
    }
    loadGlyphPack(_pack) {
        // this.renderer.load_glyph_pack(pack);
        return Promise.resolve();
    }
    createOptionsToRust(options) {
        const rustOptions = new this.rendererJs.CreateSessionOptions();
        if (options.format !== undefined) {
            rustOptions.format = options.format;
        }
        if (options.artifactContent !== undefined) {
            rustOptions.artifact_content = options.artifactContent;
        }
        return rustOptions;
    }
    canvasOptionsToRust(options) {
        const rustOptions = new this.rendererJs.RenderPageImageOptions();
        if (options.pageOffset === undefined) {
            throw new Error('pageOffset is required in reflexo v0.5.0');
        }
        else {
            rustOptions.page_off = options.pageOffset;
        }
        if (options.cacheKey !== undefined) {
            rustOptions.cache_key = options.cacheKey;
        }
        if (options.backgroundColor !== undefined) {
            rustOptions.background_color = options.backgroundColor;
        }
        if (options.pixelPerPt !== undefined) {
            rustOptions.pixel_per_pt = options.pixelPerPt;
        }
        if (options.dataSelection !== undefined) {
            let encoded = 0;
            if (options.dataSelection.body) {
                encoded |= 1 << 0;
            }
            else if (options.canvas && warnOnceCanvasSet) {
                warnOnceCanvasSet = false;
                console.warn('dataSelection.body is not set but providing canvas for body');
            }
            if (options.dataSelection.text || options.dataSelection.annotation) {
                console.error('dataSelection.text and dataSelection.annotation are deprecated');
            }
            if (options.dataSelection.semantics) {
                encoded |= 1 << 3;
            }
            rustOptions.data_selection = encoded;
        }
        return rustOptions;
    }
    retrievePagesInfoFromSession(session) {
        return session.retrievePagesInfo();
    }
    /**
     * Render a Typst document to canvas.
     */
    renderCanvas(options) {
        return this.withinOptionSession(options, async (sessionRef) => {
            return this.renderer.render_page_to_canvas(sessionRef[kObject], options.canvas || undefined, this.canvasOptionsToRust(options));
        });
    }
    // async renderPdf(artifactContent: string): Promise<Uint8Array> {
    // return this.renderer.render_to_pdf(artifactContent);
    // }
    async inAnimationFrame(fn) {
        return new Promise((resolve, reject) => {
            requestAnimationFrame(() => {
                try {
                    resolve(fn());
                }
                catch (e) {
                    reject(e);
                }
            });
        });
    }
    async renderDisplayLayer(session, canvasList, options) {
        const pages_info = session[kObject].pages_info;
        const page_count = pages_info.page_count;
        const doRender = async (i, page_off) => {
            const canvas = canvasList[i];
            const ctx = canvas.getContext('2d');
            if (!ctx) {
                throw new Error('canvas context is null');
            }
            return await this.renderCanvas({
                ...options,
                canvas: ctx,
                renderSession: session,
                pageOffset: page_off,
            });
        };
        const t = performance.now();
        const textContentList = await (async () => {
            const results = [];
            for (let i = 0; i < page_count; i++) {
                results.push(await this.inAnimationFrame(() => doRender(i, i)));
            }
            return results;
        })();
        const t2 = performance.now();
        console.log(`display layer used: render = ${(t2 - t).toFixed(1)}ms`);
        return textContentList;
    }
    renderTextLayer(layerList, textSourceList) {
        const t2 = performance.now();
        layerList.forEach((layer, i) => {
            layer.innerHTML = textSourceList[i].htmlSemantics[0];
        });
        const t3 = performance.now();
        console.log(`text layer used: render = ${(t3 - t2).toFixed(1)}ms`);
    }
    async render(options) {
        if ('format' in options) {
            if (options.format !== 'vector') {
                const artifactFormats = ['serde_json', 'js', 'ir'];
                if (artifactFormats.includes(options.format)) {
                    // deprecated
                    throw new Error(`deprecated format ${options.format}, please use vector format`);
                }
            }
        }
        return this.renderToCanvas(options);
    }
    async renderDom(options) {
        if ('format' in options) {
            if (options.format !== 'vector') {
                const artifactFormats = ['serde_json', 'js', 'ir'];
                if (artifactFormats.includes(options.format)) {
                    // deprecated
                    throw new Error(`deprecated format ${options.format}, please use vector format`);
                }
            }
        }
        return this.withinOptionSession(options, async (sessionRef) => {
            const t = new TypstDomDocument({
                ...options,
                renderMode: 'dom',
                hookedElem: options.container,
                kModule: sessionRef,
                renderer: this,
            });
            t;
            await t.impl.mountDom(options.pixelPerPt);
            return t;
        });
    }
    async renderToCanvas(options) {
        let session;
        let renderPageResults;
        const mountContainer = options.container;
        mountContainer.style.visibility = 'hidden';
        const doRenderDisplayLayer = async (canvasList, resetLayout) => {
            try {
                renderPageResults = await this.renderDisplayLayer(session, canvasList, options);
                resetLayout();
            }
            finally {
                mountContainer.style.visibility = 'visible';
            }
        };
        return this.withinOptionSession(options, async (sessionRef) => {
            session = sessionRef;
            if (session[kObject].pages_info.page_count === 0) {
                throw new Error(`No page found in session`);
            }
            if (options.pixelPerPt !== undefined && options.pixelPerPt <= 0) {
                throw new Error('Invalid typst.RenderOptions.pixelPerPt, should be a positive number ' +
                    options.pixelPerPt);
            }
            let backgroundColor = options.backgroundColor;
            if (backgroundColor !== undefined) {
                if (!/^#[0-9a-f]{6}$/.test(backgroundColor)) {
                    throw new Error('Invalid typst.backgroundColor color for matching ^#?[0-9a-f]{6}$ ' + backgroundColor);
                }
            }
            session.pixelPerPt = options.pixelPerPt ?? TypstDefaultParams.PIXEL_PER_PT;
            session.backgroundColor = backgroundColor ?? '#ffffff';
            const t = performance.now();
            const pageView = new RenderView(this.retrievePagesInfoFromSession(session), mountContainer, options);
            const t2 = performance.now();
            console.log(`layer used: retrieve = ${(t2 - t).toFixed(1)}ms`);
            await doRenderDisplayLayer(pageView.canvasList, () => pageView.resetLayout());
            this.renderTextLayer(pageView.textLayerList, renderPageResults);
            return;
        });
    }
    createModule(b) {
        return Promise.resolve(new RenderSession(this, this.renderer.create_session(b &&
            this.createOptionsToRust({
                format: 'vector',
                artifactContent: b,
            }))));
    }
    async createWorkerV0(worker) {
        return new TypstWorker(this, await this.renderer.create_worker(worker));
    }
    workerBridge() {
        return this.renderer.create_worker_bridge();
    }
    renderSvg(options, container) {
        if (options instanceof RenderSession || container) {
            throw new Error('removed api, please use renderToSvg({ renderSession, container }) instead');
        }
        return this.withinOptionSession(options, async (sessionRef) => {
            let parts = undefined;
            if (options.data_selection) {
                parts = 0;
                if (options.data_selection.body) {
                    parts |= 1 << 0;
                }
                if (options.data_selection.defs) {
                    parts |= 1 << 1;
                }
                if (options.data_selection.css) {
                    parts |= 1 << 2;
                }
                if (options.data_selection.js) {
                    parts |= 1 << 3;
                }
            }
            return Promise.resolve(this.renderer.svg_data(sessionRef[kObject], parts));
        });
    }
    renderSvgDiff(options) {
        if (!options.window) {
            return this.renderer.render_svg_diff(options.renderSession[kObject], 0, 0, 1e33, 1e33);
        }
        return this.renderer.render_svg_diff(options.renderSession[kObject], options.window.lo.x, options.window.lo.y, options.window.hi.x, options.window.hi.y);
    }
    renderToSvg(options) {
        return this.withinOptionSession(options, async (sessionRef) => {
            return Promise.resolve(this.renderer.render_svg(sessionRef[kObject], options.container));
        });
    }
    getCustomV1(options) {
        return Promise.resolve(this.renderer.get_customs(options.renderSession[kObject]));
    }
    resetSession(session) {
        return this.renderer.reset(session[kObject]);
    }
    manipulateData(opts) {
        return this.renderer.manipulate_data(opts.renderSession[kObject], opts.action ?? 'reset', opts.data);
    }
    withinOptionSession(options, fn) {
        function isRenderByContentOption(options) {
            return 'artifactContent' in options;
        }
        if ('renderSession' in options) {
            return fn(options.renderSession);
        }
        if (isRenderByContentOption(options)) {
            // todo: remove any
            return this.runWithSession(options, fn);
        }
        throw new Error('Invalid render options, should be one of RenderByContentOptions|RenderBySessionOptions');
    }
    async runWithSession(arg1, arg2) {
        let options = arg1;
        let fn = arg2;
        if (!arg2) {
            options = undefined;
            fn = arg1;
        }
        const session = this.renderer.create_session(
        /* moved */ options && this.createOptionsToRust(options));
        try {
            const res = await fn(new RenderSession(this, session));
            session.free();
            return res;
        }
        catch (e) {
            session.free();
            throw e;
        }
    }
}
//# sourceMappingURL=data:application/json;base64,eyJ2ZXJzaW9uIjozLCJmaWxlIjoicmVuZGVyZXIubWpzIiwic291cmNlUm9vdCI6IiIsInNvdXJjZXMiOlsiLi4vLi4vc3JjL3JlbmRlcmVyLm10cyJdLCJuYW1lcyI6W10sIm1hcHBpbmdzIjoiQUFJQSxPQUFPLEVBQWdDLGtCQUFrQixFQUFFLE9BQU8sRUFBRSxNQUFNLHNCQUFzQixDQUFDO0FBYWpHLE9BQU8sRUFBRSxVQUFVLEVBQUUsTUFBTSwwQkFBMEIsQ0FBQztBQUN0RCxPQUFPLEVBQUUsY0FBYyxFQUFFLE1BQU0sWUFBWSxDQUFDO0FBQzVDLE9BQU8sRUFBRSxjQUFjLEVBQUUsTUFBTSxZQUFZLENBQUM7QUFDNUMsT0FBTyxFQUFFLGdCQUFnQixFQUFFLE1BQU0sV0FBVyxDQUFDO0FBZTdDOzs7Ozs7R0FNRztBQUNILE1BQU0sT0FBTyxhQUFhO0lBYWQ7SUFaVjs7T0FFRztJQUNJLENBQUMsT0FBTyxDQUFDLENBQXNCO0lBRXRDOztPQUVHO0lBQ0g7SUFDRTs7T0FFRztJQUNLLE1BQXFCLEVBQzdCLENBQXNCO1FBRGQsV0FBTSxHQUFOLE1BQU0sQ0FBZTtRQUc3QixJQUFJLENBQUMsT0FBTyxDQUFDLEdBQUcsQ0FBQyxDQUFDO0lBQ3BCLENBQUM7SUFFRDs7Ozs7Ozs7O09BU0c7SUFDSCxJQUFJLGVBQWUsQ0FBQyxDQUFxQjtRQUN2QyxJQUFJLENBQUMsS0FBSyxTQUFTLEVBQUUsQ0FBQztZQUNwQixJQUFJLENBQUMsT0FBTyxDQUFDLENBQUMsZ0JBQWdCLEdBQUcsQ0FBQyxDQUFDO1FBQ3JDLENBQUM7SUFDSCxDQUFDO0lBRUQ7Ozs7OztPQU1HO0lBQ0gsSUFBSSxlQUFlO1FBQ2pCLE9BQU8sSUFBSSxDQUFDLE9BQU8sQ0FBQyxDQUFDLGdCQUFnQixDQUFDO0lBQ3hDLENBQUM7SUFFRDs7Ozs7O09BTUc7SUFDSCxJQUFJLFVBQVUsQ0FBQyxDQUFxQjtRQUNsQyxJQUFJLENBQUMsS0FBSyxTQUFTLEVBQUUsQ0FBQztZQUNwQixJQUFJLENBQUMsT0FBTyxDQUFDLENBQUMsWUFBWSxHQUFH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IEB0cy1pZ25vcmVcbmltcG9ydCB0eXBlICogYXMgdHlwc3QgZnJvbSAnQG15cmlhZGRyZWFtaW4vdHlwc3QtdHMtcmVuZGVyZXInO1xuXG5pbXBvcnQgdHlwZSB7IEluaXRPcHRpb25zIH0gZnJvbSAnLi9vcHRpb25zLmluaXQubWpzJztcbmltcG9ydCB7IFBhZ2VJbmZvLCBSZW5kZXJDYW52YXNSZXN1bHQsIFR5cHN0RGVmYXVsdFBhcmFtcywga09iamVjdCB9IGZyb20gJy4vaW50ZXJuYWwudHlwZXMubWpzJztcbmltcG9ydCB7XG4gIENyZWF0ZVNlc3Npb25PcHRpb25zLFxuICBSZW5kZXJUb0NhbnZhc09wdGlvbnMsXG4gIFJlbmRlck9wdGlvbnMsXG4gIFJlbmRlckNhbnZhc09wdGlvbnMsXG4gIFJlbmRlclRvU3ZnT3B0aW9ucyxcbiAgTWFuaXB1bGF0ZURhdGFPcHRpb25zLFxuICBSZW5kZXJTdmdPcHRpb25zLFxuICBSZW5kZXJJblNlc3Npb25PcHRpb25zLFxuICBNb3VudERvbU9wdGlvbnMsXG4gIE9mZnNjcmVlblJlbmRlckNhbnZhc09wdGlvbnMsXG59IGZyb20gJy4vb3B0aW9ucy5yZW5kZXIubWpzJztcbmltcG9ydCB7IFJlbmRlclZpZXcgfSBmcm9tICcuL3JlbmRlci9jYW52YXMvdmlldy5tanMnO1xuaW1wb3J0IHsgTGF6eVdhc21Nb2R1bGUgfSBmcm9tICcuL3dhc20ubWpzJztcbmltcG9ydCB7IGJ1aWxkQ29tcG9uZW50IH0gZnJvbSAnLi9pbml0Lm1qcyc7XG5pbXBvcnQgeyBUeXBzdERvbURvY3VtZW50IH0gZnJvbSAnLi9kb20ubWpzJztcblxuLyoqXG4gKiBUaGUgcmVzdWx0IG9mIHJlbmRlcmluZyBhIFR5cHN0IGRvY3VtZW50LlxuICogQHR5cGVkZWYge09iamVjdH0gUmVuZGVyUmVzdWx0XG4gKiBAcHJvcGVydHkge251bWJlcn0gd2lkdGggLSBUaGUgd2lkdGggb2YgdGhlIHJlbmRlcmVkIFR5cHN0IGRvY3VtZW50IChzaW5nbGUgcGFnZSkuXG4gKiBAcHJvcGVydHkge251bWJlcn0gaGVpZ2h0IC0gVGhlIGhlaWdodCBvZiB0aGUgcmVuZGVyZWQgVHlwc3QgZG9jdW1lbnQgKHNpbmdsZSBwYWdlKS5cbiAqL1xuZXhwb3J0IGludGVyZmFjZSBSZW5kZXJSZXN1bHQge1xuICB3aWR0aDogbnVtYmVyO1xuICBoZWlnaHQ6IG51bWJlcjtcbn1cblxudHlwZSBDb250ZXh0ZWRSZW5kZXJPcHRpb25zPFQ+ID0gVCB8IFJlbmRlck9wdGlvbnM8VD47XG5cbi8qKlxuICogVGhlIHNlc3Npb24gb2YgYSBUeXBzdCBkb2N1bWVudC5cbiAqIEBwcm9wZXJ0eSB7c3RyaW5nfSBiYWNrZ3JvdW5kQ29sb3IgLSBUaGUgYmFja2dyb3VuZCBjb2xvciBvZiB0aGUgVHlwc3RcbiAqIGRvY3VtZW50LlxuICogQHByb3BlcnR5IHtudW1iZXJ9IHBpeGVsUGVyUHQgLSBUaGUgcGl4ZWwgcGVyIHBvaW50IHNjYWxlIHVwIHRoZSBpbWFnZS5cbiAqXG4gKi9cbmV4cG9ydCBjbGFzcyBSZW5kZXJTZXNzaW9uIHtcbiAgLyoqXG4gICAqIEBpbnRlcm5hbFxuICAgKi9cbiAgcHVibGljIFtrT2JqZWN0XTogdHlwc3QuUmVuZGVyU2Vzc2lvbjtcblxuICAvKipcbiAgICogQGludGVybmFsXG4gICAqL1xuICBjb25zdHJ1Y3RvcihcbiAgICAvKipcbiAgICAgKiBAaW50ZXJuYWxcbiAgICAgKi9cbiAgICBwcml2YXRlIHBsdWdpbjogVHlwc3RSZW5kZXJlcixcbiAgICBvOiB0eXBzdC5SZW5kZXJTZXNzaW9uLFxuICApIHtcbiAgICB0aGlzW2tPYmplY3RdID0gbztcbiAgfVxuXG4gIC8qKlxuICAgKiBAZGVwcmVjYXRlZCBzZXQgaW4ge0BsaW5rIFJlbmRlclRvQ2FudmFzT3B0aW9uc30gaW5zdGVhZFxuICAgKlxuICAgKiBTZXQgdGhlIGJhY2tncm91bmQgY29sb3Igb2YgdGhlIFR5cHN0IGRvY3VtZW50LlxuICAgKiBAcGFyYW0ge3N0cmluZ30gdCAtIFRoZSBiYWNrZ3JvdW5kIGNvbG9yIGluIGZvcm1hdCBvZiBgXiM/WzAtOWEtZl17Nn0kYFxuICAgKlxuICAgKiBOb3RlOiBEZWZhdWx0IHRvIGAjZmZmZmZmYC5cbiAgICpcbiAgICogTm90ZTogT25seSBhdmFpbGFibGUgaW4gY2FudmFzIHJlbmRlcmluZyBtb2RlLlxuICAgKi9cbiAgc2V0IGJhY2tncm91bmRDb2xvcih0OiBzdHJpbmcgfCB1bmRlZmluZWQpIHtcbiAgICBpZiAodCAhPT0gdW5kZWZpbmVkKSB7XG4gICAgICB0aGlzW2tPYmplY3RdLmJhY2tncm91bmRfY29sb3IgPSB0O1xuICAgIH1cbiAgfVxuXG4gIC8qKlxuICAgKiBHZXQgdGhlIGJhY2tncm91bmQgY29sb3Igb2YgdGhlIFR5cHN0IGRvY3VtZW50LlxuICAgKlxuICAgKiBOb3RlOiBEZWZhdWx0IHRvIGAjZmZmZmZmYC5cbiAgICpcbiAgICogTm90ZTogT25seSBhdmFpbGFibGUgaW4gY2FudmFzIHJlbmRlcmluZyBtb2RlLlxuICAgKi9cbiAgZ2V0IGJhY2tncm91bmRDb2xvcigpOiBzdHJpbmcgfCB1bmRlZmluZWQge1xuICAgIHJldHVybiB0aGlzW2tPYmplY3RdLmJhY2tncm91bmRfY29sb3I7XG4gIH1cblxuICAvKipcbiAgICogU2V0IHRoZSBwaXhlbCBwZXIgcG9pbnQgc2NhbGUgdXAgdGhlIGNhbnZhcyBwYW5lbC5cbiAgICpcbiAgICogTm90ZTogRGVmYXVsdCB0byBgM2AuXG4gICAqXG4gICAqIE5vdGU6IE9ubHkgYXZhaWxhYmxlIGluIGNhbnZhcyByZW5kZXJpbmcgbW9kZS5cbiAgICovXG4gIHNldCBwaXhlbFBlclB0KHQ6IG51bWJlciB8IHVuZGVmaW5lZCkge1xuICAgIGlmICh0ICE9PSB1bmRlZmluZWQpIHtcbiAgICAgIHRoaXNba09iamVjdF0ucGl4ZWxfcGVyX3B0ID0gdDtcbiAgICB9XG4gIH1cblxuICAvKipcbiAgICogQGRlcHJlY2F0ZWQgc2V0IGluIHtAbGluayBSZW5kZXJUb0NhbnZhc09wdGlvbnN9IGluc3RlYWRcbiAgICpcbiAgICogR2V0IHRoZSBwaXhlbCBwZXIgcG9pbnQgc2NhbGUgdXAgdGhlIGNhbnZhcyBwYW5lbC5cbiAgICpcbiAgICogTm90ZTogRGVmYXVsdCB0byBgM2AuXG4gICAqXG4gICAqIE5vdGU6IE9ubHkgYXZhaWxhYmxlIGluIGNhbnZhcyByZW5kZXJpbmcgbW9kZS5cbiAgICovXG4gIGdldCBwaXhlbFBlclB0KCk6IG51bWJlciB8IHVuZGVmaW5lZCB7XG4gICAgcmV0dXJuIHRoaXNba09iamVjdF0ucGl4ZWxfcGVyX3B0O1xuICB9XG5cbiAgLyoqXG4gICAqIFJlc2V0IHN0YXRlXG4gICAqL1xuICByZXNldCgpOiB2b2lkIHtcbiAgICB0aGlzLnBsdWdpbi5yZXNldFNlc3Npb24odGhpcyk7XG4gIH1cblxuICAvKipcbiAgICogQGRlcHJlY2F0ZWRcbiAgICogdXNlIHtAbGluayBkb2NXaWR0aH0gaW5zdGVhZFxuICAgKi9cbiAgZ2V0IGRvY193aWR0aCgpOiBudW1iZXIge1xuICAgIHJldHVybiB0aGlzW2tPYmplY3RdLmRvY193aWR0aDtcbiAgfVxuXG4gIGdldCBkb2NXaWR0aCgpOiBudW1iZXIge1xuICAgIHJldHVybiB0aGlzW2tPYmplY3RdLmRvY193aWR0aDtcbiAgfVxuXG4gIC8qKlxuICAgKiBAZGVwcmVjYXRlZFxuICAgKiB1c2Uge0BsaW5rIGRvY0hlaWdodH0gaW5zdGVhZFxuICAgKi9cbiAgZ2V0IGRvY19oZWlnaHQoKTogbnVtYmVyIHtcbiAgICByZXR1cm4gdGhpc1trT2JqZWN0XS5kb2NfaGVpZ2h0O1xuICB9XG5cbiAgZ2V0IGRvY0hlaWdodCgpOiBudW1iZXIge1xuICAgIHJldHVybiB0aGlzW2tPYmplY3RdLmRvY19oZWlnaHQ7XG4gIH1cblxuICByZXRyaWV2ZVBhZ2VzSW5mbygpOiBQYWdlSW5mb1tdIHtcbiAgICBjb25zdCBwYWdlc19pbmZvID0gdGhpc1trT2JqZWN0XS5wYWdlc19pbmZvO1xuICAgIGNvbnN0IHBhZ2VJbmZvczogUGFnZUluZm9bXSA9IFtdO1xuICAgIGNvbnN0IHBhZ2VDb3VudCA9IHBhZ2VzX2luZm8ucGFnZV9jb3VudDtcbiAgICBmb3IgKGxldCBpID0gMDsgaSA8IHBhZ2VDb3VudDsgaSsrKSB7XG4gICAgICBjb25zdCBwYWdlQXN0ID0gcGFnZXNfaW5mby5wYWdlKGkpO1xuICAgICAgcGFnZUluZm9zLnB1c2goe1xuICAgICAgICBwYWdlT2Zmc2V0OiBwYWdlQXN0LnBhZ2Vfb2ZmLFxuICAgICAgICB3aWR0aDogcGFnZUFzdC53aWR0aF9wdCxcbiAgICAgICAgaGVpZ2h0OiBwYWdlQXN0LmhlaWdodF9wdCxcbiAgICAgIH0pO1xuICAgIH1cblxuICAgIHJldHVybiBwYWdlSW5mb3M7XG4gIH1cblxuICBnZXRTb3VyY2VMb2MocGF0aDogVWludDMyQXJyYXkpOiBzdHJpbmcgfCB1bmRlZmluZWQge1xuICAgIHJldHVybiAodGhpc1trT2JqZWN0XSBhcyB0eXBzdC5SZW5kZXJTZXNzaW9uKS5zb3VyY2Vfc3BhbihwYXRoKTtcbiAgfVxuXG4gIC8qKlxuICAgKiBTZWUge0BsaW5rIFR5cHN0UmVuZGVyZXIjcmVuZGVyU3ZnfSBmb3IgbW9yZSBkZXRhaWxzLlxuICAgKi9cbiAgcmVuZGVyU3ZnKG9wdGlvbnM6IENvbnRleHRlZFJlbmRlck9wdGlvbnM8UmVuZGVyU3ZnT3B0aW9ucz4pOiBQcm9taXNlPHN0cmluZz4ge1xuICAgIHJldHVybiB0aGlzLnBsdWdpbi5yZW5kZXJTdmcoe1xuICAgICAgcmVuZGVyU2Vzc2lvbjogdGhpcyxcbiAgICAgIC4uLm9wdGlvbnMsXG4gICAgfSk7XG4gIH1cblxuICAvKipcbiAgICogU2VlIHtAbGluayBUeXBzdFJlbmRlcmVyI3JlbmRlclRvU3ZnfSBmb3IgbW9yZSBkZXRhaWxzLlxuICAgKi9cbiAgcmVuZGVyVG9Tdmcob3B0aW9uczogQ29udGV4dGVkUmVuZGVyT3B0aW9uczxSZW5kZXJUb1N2Z09wdGlvbnM+KTogUHJvbWlzZTxib29sZWFuPiB7XG4gICAgcmV0dXJuIHRoaXMucGx1Z2luLnJlbmRlclRvU3ZnKHtcbiAgICAgIHJlbmRlclNlc3Npb246IHRoaXMsXG4gICAgICAuLi5vcHRpb25zLFxuICAgIH0pO1xuICB9XG5cbiAgLyoqXG4gICAqIFNlZSB7QGxpbmsgVHlwc3RSZW5kZXJlciNyZW5kZXJDYW52YXN9IGZvciBtb3JlIGRldGFpbHMuXG4gICAqL1xuICByZW5kZXJDYW52YXMob3B0aW9uczogQ29udGV4dGVkUmVuZGVyT3B0aW9uczxSZW5kZXJDYW52YXNPcHRpb25zPik6IFByb21pc2U8UmVuZGVyQ2FudmFzUmVzdWx0PiB7XG4gICAgcmV0dXJuIHRoaXMucGx1Z2luLnJlbmRlckNhbnZhcyh7XG4gICAgICByZW5kZXJTZXNzaW9uOiB0aGlzLFxuICAgICAgLi4ub3B0aW9ucyxcbiAgICB9KTtcbiAgfVxuXG4gIC8qKlxuICAgKiBTZWUge0BsaW5rIFR5cHN0UmVuZGVyZXIjbWFuaXB1bGF0ZURhdGF9IGZvciBtb3JlIGRldGFpbHMuXG4gICAqL1xuICBtYW5pcHVsYXRlRGF0YShvcHRzOiBNYW5pcHVsYXRlRGF0YU9wdGlvbnMpIHtcbiAgICB0aGlzLnBsdWdpbi5tYW5pcHVsYXRlRGF0YSh7XG4gICAgICByZW5kZXJTZXNzaW9uOiB0aGlzLFxuICAgICAgLi4ub3B0cyxcbiAgICB9KTtcbiAgfVxuXG4gIC8qKlxuICAgKiBTZWUge0BsaW5rIFR5cHN0UmVuZGVyZXIjcmVuZGVyU3ZnRGlmZn0gZm9yIG1vcmUgZGV0YWlscy5cbiAgICovXG4gIHJlbmRlclN2Z0RpZmYob3B0czogUmVuZGVyU3ZnT3B0aW9ucyk6IHN0cmluZyB7XG4gICAgcmV0dXJuIHRoaXMucGx1Z2luLnJlbmRlclN2Z0RpZmYoe1xuICAgICAgcmVuZGVyU2Vzc2lvbjogdGhpcyxcbiAgICAgIC4uLm9wdHMsXG4gICAgfSk7XG4gIH1cblxuICAvKipcbiAgICogQGRlcHJlY2F0ZWRcbiAgICogdXNlIHtAbGluayBnZXRTb3VyY2VMb2N9IGluc3RlYWRcbiAgICovXG4gIGdldF9zb3VyY2VfbG9jKHBhdGg6IFVpbnQzMkFycmF5KTogc3RyaW5nIHwgdW5kZWZpbmVkIHtcbiAgICByZXR1cm4gKHRoaXNba09iamVjdF0gYXMgdHlwc3QuUmVuZGVyU2Vzc2lvbikuc291cmNlX3NwYW4ocGF0aCk7XG4gIH1cblxuICAvKipcbiAgICogQGRlcHJlY2F0ZWRcbiAgICogdXNlIHtAbGluayByZW5kZXJTdmdEaWZmfSBpbnN0ZWFkXG4gICAqL1xuICByZW5kZXJfaW5fd2luZG93KHJlY3RfbG9feDogbnVtYmVyLCByZWN0X2xvX3k6IG51bWJlciwgcmVjdF9oaV94OiBudW1iZXIsIHJlY3RfaGlfeTogbnVtYmVyKSB7XG4gICAgcmV0dXJuIHRoaXNba09iamVjdF0ucmVuZGVyX2luX3dpbmRvdyhyZWN0X2xvX3gsIHJlY3RfbG9feSwgcmVjdF9oaV94LCByZWN0X2hpX3kpO1xuICB9XG5cbiAgLyoqXG4gICAqIEBkZXByZWNhdGVkXG4gICAqIHVzZSB7QGxpbmsgbWFuaXB1bGF0ZURhdGF9IGluc3RlYWRcbiAgICovXG4gIG1lcmdlX2RlbHRhKGRhdGE6IFVpbnQ4QXJyYXkpIHtcbiAgICB0aGlzLnBsdWdpbi5tYW5pcHVsYXRlRGF0YSh7XG4gICAgICByZW5kZXJTZXNzaW9uOiB0aGlzLFxuICAgICAgYWN0aW9uOiAnbWVyZ2UnLFxuICAgICAgZGF0YSxcbiAgICB9KTtcbiAgfVxufVxuXG4vKipcbiAqIEBpbnRlcm5hbFxuICovXG5jb25zdCBlbnVtIE1hbmFnZVN0YXR1cyB7XG4gIERlbGV0ZSxcbiAgTmV3LFxuICBVcGRhdGUsXG59XG5cbmV4cG9ydCBjbGFzcyBUeXBzdFdvcmtlciB7XG4gIC8qKlxuICAgKiBAaW50ZXJuYWxcbiAgICovXG4gIHB1YmxpYyBba09iamVjdF06IHR5cHN0LlR5cHN0V29ya2VyO1xuXG4gIC8qKlxuICAgKiBAaW50ZXJuYWxcbiAgICovXG4gIGNvbnN0cnVjdG9yKFxuICAgIC8qKlxuICAgICAqIEBpbnRlcm5hbFxuICAgICAqL1xuICAgIHByaXZhdGUgcGx1Z2luOiBUeXBzdFJlbmRlcmVyRHJpdmVyLFxuICAgIG86IHR5cHN0LlR5cHN0V29ya2VyLFxuICApIHtcbiAgICB0aGlzW2tPYmplY3RdID0gbztcbiAgfVxuXG4gIC8qKlxuICAgKiBTZWUge0BsaW5rIFR5cHN0UmVuZGVyZXIjbWFuaXB1bGF0ZURhdGF9IGZvciBtb3JlIGRldGFpbHMuXG4gICAqL1xuICBtYW5pcHVsYXRlRGF0YShhY3Rpb246IHN0cmluZywgZGF0YTogVWludDhBcnJheSk6IFByb21pc2U8dm9pZD4ge1xuICAgIHJldHVybiB0aGlzW2tPYmplY3RdLm1hbmlwdWxhdGVfZGF0YShhY3Rpb24sIGRhdGEpO1xuICB9XG5cbiAgLyoqXG4gICAqIEBpbnRlcm5hbFxuICAgKi9cbiAgbWFuYWdlZENhbnZhc0VsZW1MaXN0ID0gbmV3IE1hcDxzdHJpbmcsIFtNYW5hZ2VTdGF0dXMsIE9mZnNjcmVlblJlbmRlckNhbnZhc09wdGlvbnNdPigpO1xuICAvKipcbiAgICogQGludGVybmFsXG4gICAqL1xuICBjYW52YXNDb3VudGVyID0gTWF0aC5yYW5kb20oKTtcbiAgLyoqXG4gICAqIFlvdSBtdXN0IHN1Ym1pdCBhbGwgY2FudmFzIGluIHBhZ2VzIHRvIGVuc3VyZSBzeW5jaHJvbml6YXRpb24gd2l0aCB0aGUgYmFja2dyb3VuZCB3b3JrZXJcbiAgICpcbiAgICogU2VlIHtAbGluayBUeXBzdFJlbmRlcmVyI3JlbmRlckNhbnZhc30gZm9yIG1vcmUgZGV0YWlscy5cbiAgICovXG4gIHJlbmRlckNhbnZhcyhjYW52YXNFbGVtTGlzdDogT2Zmc2NyZWVuUmVuZGVyQ2FudmFzT3B0aW9uc1tdKTogUHJvbWlzZTxSZW5kZXJDYW52YXNSZXN1bHRbXT4ge1xuICAgIGNvbnN0IG0gPSB0aGlzLm1hbmFnZWRDYW52YXNFbGVtTGlzdDtcbiAgICBmb3IgKGNvbnN0IFtfLCBlbGVtXSBvZiBtKSB7XG4gICAgICBlbGVtWzBdID0gTWFuYWdlU3RhdHVzLkRlbGV0ZTtcbiAgICB9XG5cbiAgICBmb3IgKGNvbnN0IGVsZW0gb2YgY2FudmFzRWxlbUxpc3QpIHtcbiAgICAgIGNvbnN0IGNhbnZhcyA9IGVsZW0uY2FudmFzO1xuICAgICAgbGV0IGVsZW1JZCA9IGNhbnZhcy5kYXRhc2V0Lm1hbmFnZUlkO1xuICAgICAgbGV0IGFjdGlvbiA9IE1hbmFnZVN0YXR1cy5VcGRhdGU7XG4gICAgICBpZiAoIWVsZW1JZCkge1xuICAgICAgICBlbGVtSWQgPSB0aGlzLmNhbnZhc0NvdW50ZXIudG9GaXhlZCg1KTtcbiAgICAgICAgdGhpcy5jYW52YXNDb3VudGVyICs9IDE7XG4gICAgICAgIGNhbnZhcy5kYXRhc2V0Lm1hbmFnZUlkID0gZWxlbUlkO1xuICAgICAgICBhY3Rpb24gPSBNYW5hZ2VTdGF0dXMuTmV3O1xuICAgICAgfVxuXG4gICAgICBsZXQgcHJldiA9IG0uZ2V0KGVsZW1JZCk7XG4gICAgICBpZiAocHJldiAmJiBwcmV2WzBdICE9PSBNYW5hZ2VTdGF0dXMuRGVsZXRlKSB7XG4gICAgICAgIHRocm93IG5ldyBFcnJvcignY2Fubm90IHVwZGF0ZSBhIGNhbnZhcyBmb3IgdHdvIHRpbWVzIGluIGJhdGNoJyk7XG4gICAgICB9XG5cbiAgICAgIG0uc2V0KGVsZW1JZCwgW2FjdGlvbiwgeyAuLi5lbGVtIH1dKTtcbiAgICB9XG5cbiAgICBjb25zdCBlbnRyaWVzID0gQXJyYXkuZnJvbShtLmVudHJpZXMoKSk7XG4gICAgY29uc3QgYWN0aW9ucyA9IG5ldyBVaW50OEFycmF5KGVudHJpZXMubGVuZ3RoKTtcbiAgICBjb25zdCBlbGVtZW50cyA9IG5ldyBBcnJheShlbnRyaWVzLmxlbmd0aCk7XG4gICAgY29uc3Qgb3B0aW9ucyA9IGVudHJpZXMubWFwKChba2V5LCBbYWN0aW9uLCBlbGVtXV0sIGluZGV4KSA9PiB7XG4gICAgICBpZiAoIWFjdGlvbikge1xuICAgICAgICBtLmRlbGV0ZShrZXkpO1xuICAgICAgfVxuXG4gICAgICBhY3Rpb25zW2luZGV4XSA9IGFjdGlvbjtcbiAgICAgIGVsZW1lbnRzW2luZGV4XSA9IGVsZW0uY2FudmFzO1xuICAgICAgcmV0dXJuIHRoaXMucGx1Z2luLmNhbnZhc09wdGlvbnNUb1J1c3QoZWxlbSk7XG4gICAgfSk7XG5cbiAgICByZXR1cm4gdGhpc1trT2JqZWN0XS5yZW5kZXJfY2FudmFzKGFjdGlvbnMsIGVsZW1lbnRzLCBvcHRpb25zKTtcbiAgfVxuXG4gIGFzeW5jIHJldHJpZXZlUGFnZXNJbmZvKCk6IFByb21pc2U8UGFnZUluZm9bXT4ge1xuICAgIGNvbnN0IHBhZ2VzX2luZm8gPSBhd2FpdCB0aGlzW2tPYmplY3RdLmdldF9wYWdlc19pbmZvKCk7XG4gICAgY29uc29sZS5sb2cocGFnZXNfaW5mbyk7XG4gICAgY29uc3QgcGFnZUluZm9zOiBQYWdlSW5mb1tdID0gW107XG4gICAgY29uc3QgcGFnZUNvdW50ID0gcGFnZXNfaW5mby5wYWdlX2NvdW50O1xuICAgIGZvciAobGV0IGkgPSAwOyBpIDwgcGFnZUNvdW50OyBpKyspIHtcbiAgICAgIGNvbnN0IHBhZ2VBc3QgPSBwYWdlc19pbmZvLnBhZ2UoaSk7XG4gICAgICBwYWdlSW5mb3MucHVzaCh7XG4gICAgICAgIHBhZ2VPZmZzZXQ6IHBhZ2VBc3QucGFnZV9vZmYsXG4gICAgICAgIHdpZHRoOiBwYWdlQXN0LndpZHRoX3B0LFxuICAgICAgICBoZWlnaHQ6IHBhZ2VBc3QuaGVpZ2h0X3B0LFxuICAgICAgfSk7XG4gICAgfVxuXG4gICAgcmV0dXJuIHBhZ2VJbmZvcztcbiAgfVxufVxuXG4vKipcbiAqIEBkZXByZWNhdGVkXG4gKiB1c2Uge0BsaW5rIFR5cHN0UmVuZGVyZXJ9IGluc3RlYWRcbiAqL1xuZXhwb3J0IGludGVyZmFjZSBUeXBzdFN2Z1JlbmRlcmVyIHtcbiAgLyoqXG4gICAqIFJlbmRlciBhIFR5cHN0IGRvY3VtZW50IHRvIHN2Zy5cbiAgICogQHBhcmFtIHtSZW5kZXJPcHRpb25zPFJlbmRlclRvU3ZnT3B0aW9ucz59IG9wdGlvbnMgLSBUaGUgb3B0aW9ucyBmb3JcbiAgICogcmVuZGVyaW5nIGEgVHlwc3QgZG9jdW1lbnQgdG8gc3BlY2lmaWVkIGNvbnRhaW5lci5cbiAgICogQHJldHVybnMge3ZvaWR9IC0gVGhlIHJlc3VsdCBvZiByZW5kZXJpbmcgYSBUeXBzdCBkb2N1bWVudC5cbiAgICogQGV4YW1wbGVcbiAgICogYGBgdHlwZXNjcmlwdFxuICAgKiBsZXQgZmV0Y2hEb2MgPSAocGF0aCkgPT4gZmV0Y2gocGF0aCkudGhlbihcbiAgICogICByZXNwb25zZSA9PiBuZXcgVWludDhBcnJheShyZXNwb25zZS5hcnJheUJ1ZmZlcigpKSlcbiAgICogcmVuZGVyZXIucmVuZGVyVG9Tdmcoe1xuICAgKiAgIGNvbnRhaW5lcjogZG9jdW1lbnQuZ2V0RWxlbWVudEJ5SWQoJ2NvbnRhaW5lcicpLFxuICAgKiAgIGFydGlmYWN0Q29udGVudDogYXdhaXQgZmV0Y2hEb2MoJ3R5cHN0LW1haW4uc2lyLmluJyksXG4gICAqIH0pO1xuICAgKiBgYGBcbiAgICovXG4gIHJlbmRlclRvU3ZnKG9wdGlvbnM6IFJlbmRlck9wdGlvbnM8UmVuZGVyVG9TdmdPcHRpb25zPik6IFByb21pc2U8Ym9vbGVhbj47XG59XG5cblxuLyoqXG4gKiBUaGUgaW50ZXJmYWNlIG9mIFR5cHN0IHJlbmRlcmVyLlxuICovXG5leHBvcnQgaW50ZXJmYWNlIFR5cHN0UmVuZGVyZXIgZXh0ZW5kcyBUeXBzdFN2Z1JlbmRlcmVyIHtcbiAgLyoqXG4gICAqIEluaXRpYWxpemUgdGhlIHR5cHN0IHJlbmRlcmVyLlxuICAgKiBAcGFyYW0ge1BhcnRpYWw8SW5pdE9wdGlvbnM+fSBvcHRpb25zIC0gVGhlIG9wdGlvbnMgZm9yIGluaXRpYWxpemluZyB0aGVcbiAgICogdHlwc3QgcmVuZGVyZXIuXG4gICAqL1xuICBpbml0KG9wdGlvbnM/OiBQYXJ0aWFsPEluaXRPcHRpb25zPik6IFByb21pc2U8dm9pZD47XG5cbiAgLyoqXG4gICAqIExvYWQgYSBnbHlwaCBwYWNrIGZvciBhbGwgb2YgdGhlIFR5cHN0IGRvY3VtZW50cyB0byByZW5kZXIuXG4gICAqIE5vdGU6IHRoaXMgZnVuY3Rpb24gaXMgc3RpbGwgdW5kZXIgZGV2ZWxvcG1lbnQuXG4gICAqIEBwYXJhbSBwYWNrXG4gICAqL1xuICBsb2FkR2x5cGhQYWNrKHBhY2s6IHVua25vd24pOiBQcm9taXNlPHZvaWQ+O1xuXG4gIC8qKlxuICAgKiBSZXNldCBzdGF0ZVxuICAgKi9cbiAgcmVzZXRTZXNzaW9uKHNlc3Npb246IFJlbmRlclNlc3Npb24pOiB2b2lkO1xuXG4gIC8qKlxuICAgKiBSZXRyaWV2ZSBwYWdlIGluZm9ybWF0aW9uIG9mIGN1cnJlbnQgc2VsZWN0ZWQgZG9jdW1lbnRcbiAgICovXG4gIHJldHJpZXZlUGFnZXNJbmZvRnJvbVNlc3Npb24oc2Vzc2lvbjogUmVuZGVyU2Vzc2lvbik6IFBhZ2VJbmZvW107XG5cbiAgLyoqXG4gICAqIFJlbmRlciBhIFR5cHN0IGRvY3VtZW50IHRvIGNhbnZhcy5cbiAgICovXG4gIHJlbmRlckNhbnZhcyhvcHRpb25zOiBSZW5kZXJPcHRpb25zPFJlbmRlckNhbnZhc09wdGlvbnM+KTogUHJvbWlzZTxSZW5kZXJDYW52YXNSZXN1bHQ+O1xuXG4gIC8qKlxuICAgKiBSZW5kZXIgYSBUeXBzdCBkb2N1bWVudCB0byBjYW52YXMuXG4gICAqIEBwYXJhbSB7UmVuZGVyT3B0aW9uczxSZW5kZXJUb0NhbnZhc09wdGlvbnM+fSBvcHRpb25zIC0gVGhlIG9wdGlvbnMgZm9yXG4gICAqIHJlbmRlcmluZyBhIFR5cHN0IGRvY3VtZW50IHRvIHNwZWNpZmllZCBjb250YWluZXIuXG4gICAqIEByZXR1cm5zIHt2b2lkfSAtIFRoZSByZXN1bHQgb2YgcmVuZGVyaW5nIGEgVHlwc3QgZG9jdW1lbnQuXG4gICAqIEBleGFtcGxlXG4gICAqIGBgYHR5cGVzY3JpcHRcbiAgICogbGV0IGZldGNoRG9jID0gKHBhdGgpID0+IGZldGNoKHBhdGgpLnRoZW4oXG4gICAqICAgcmVzcG9uc2UgPT4gbmV3IFVpbnQ4QXJyYXkocmVzcG9uc2UuYXJyYXlCdWZmZXIoKSkpXG4gICAqIHJlbmRlcmVyLnJlbmRlclRvQ2FudmFzKHtcbiAgICogICBjb250YWluZXI6IGRvY3VtZW50LmdldEVsZW1lbnRCeUlkKCdjb250YWluZXInKSxcbiAgICogICBwaXhlbFBlclB0OiAzLFxuICAgKiAgIGJhY2tncm91bmRDb2xvcjogJyNmZmZmZmYnLFxuICAgKiAgIGFydGlmYWN0Q29udGVudDogYXdhaXQgZmV0Y2hEb2MoJ3R5cHN0LW1haW4uc2lyLmluJyksXG4gICAqIH0pO1xuICAgKiBgYGBcbiAgICovXG4gIHJlbmRlclRvQ2FudmFzKG9wdGlvbnM6IFJlbmRlck9wdGlvbnM8UmVuZGVyVG9DYW52YXNPcHRpb25zPik6IFByb21pc2U8dm9pZD47XG5cbiAgLyoqXG4gICAqIFJlbmRlciBhIFR5cHN0IGRvY3VtZW50IHRvIChub24taW5jcmVtZW50YWwpIHN2ZyBzdHJpbmcuXG4gICAqIEBwYXJhbSB7UmVuZGVyT3B0aW9uczxSZW5kZXJTdmdPcHRpb25zPn0gb3B0aW9ucyAtIFRoZSBvcHRpb25zIGZvclxuICAgKiByZW5kZXJpbmcgYSBUeXBzdCBkb2N1bWVudCB0byBzcGVjaWZpZWQgY29udGFpbmVyLlxuICAgKiBAcmV0dXJucyB7c3RyaW5nfSAtIFRoZSByZW5kZXJlZCBjb250ZW50LlxuICAgKiBAZXhhbXBsZVxuICAgKiBgYGB0eXBlc2NyaXB0XG4gICAqIGxldCBmZXRjaERvYyA9IChwYXRoKSA9PiBmZXRjaChwYXRoKS50aGVuKFxuICAgKiAgIHJlc3BvbnNlID0+IG5ldyBVaW50OEFycmF5KHJlc3BvbnNlLmFycmF5QnVmZmVyKCkpKVxuICAgKiBjb25zdCBzdmcgPSByZW5kZXJlci5yZW5kZXJTdmcoe1xuICAgKiAgIGFydGlmYWN0Q29udGVudDogYXdhaXQgZmV0Y2hEb2MoJ3R5cHN0LW1haW4uc2lyLmluJyksXG4gICAqIH0pO1xuICAgKiBgYGBcbiAgICovXG4gIHJlbmRlclN2ZyhvcHRpb25zOiBSZW5kZXJPcHRpb25zPFJlbmRlclN2Z09wdGlvbnM+KTogUHJvbWlzZTxzdHJpbmc+O1xuXG4gIC8qKlxuICAgKiBSZW5kZXIgYSBUeXBzdCBkb2N1bWVudCB0byBzdmcuXG4gICAqIEBwYXJhbSB7UmVuZGVyT3B0aW9uczxSZW5kZXJUb1N2Z09wdGlvbnM+fSBvcHRpb25zIC0gVGhlIG9wdGlvbnMgZm9yXG4gICAqIHJlbmRlcmluZyBhIFR5cHN0IGRvY3VtZW50IHRvIHNwZWNpZmllZCBjb250YWluZXIuXG4gICAqIEBleGFtcGxlXG4gICAqIGBgYHR5cGVzY3JpcHRcbiAgICogbGV0IGZldGNoRG9jID0gKHBhdGgpID0+IGZldGNoKHBhdGgpLnRoZW4oXG4gICAqICAgcmVzcG9uc2UgPT4gbmV3IFVpbnQ4QXJyYXkocmVzcG9uc2UuYXJyYXlCdWZmZXIoKSkpXG4gICAqIHJlbmRlcmVyLnJlbmRlclRvU3ZnKHtcbiAgICogICBjb250YWluZXI6IGRvY3VtZW50LmdldEVsZW1lbnRCeUlkKCdjb250YWluZXInKSxcbiAgICogICBhcnRpZmFjdENvbnRlbnQ6IGF3YWl0IGZldGNoRG9jKCd0eXBzdC1tYWluLnNpci5pbicpLFxuICAgKiB9KTtcbiAgICogYGBgXG4gICAqL1xuICByZW5kZXJUb1N2ZyhvcHRpb25zOiBSZW5kZXJPcHRpb25zPFJlbmRlclRvU3ZnT3B0aW9ucz4pOiBQcm9taXNlPGJvb2xlYW4+O1xuXG4gIC8qKlxuICAgKiBSZXR1cm4gc2VsZWN0ZWQgY3VycmVudCBzdmdcbiAgICovXG4gIGdldEN1c3RvbVYxKG9wdGlvbnM6IFJlbmRlckluU2Vzc2lvbk9wdGlvbnM8e30+KTogUHJvbWlzZTxhbnk+O1xuXG4gIC8qKlxuICAgKiBleHBlcmltZW50YWxcbiAgICovXG4gIHJlbmRlclN2Z0RpZmYob3B0aW9uczogUmVuZGVySW5TZXNzaW9uT3B0aW9uczxSZW5kZXJTdmdPcHRpb25zPik6IHN0cmluZztcblxuICAvKipcbiAgICogTWFuaXB1bGF0ZSB0aGUgVHlwc3QgZG9jdW1lbnQgaW4gdGhlIHNlc3Npb24uXG4gICAqIFNlZSB7QGxpbmsgTWFuaXB1bGF0ZURhdGFPcHRpb25zfSBmb3IgbW9yZSBkZXRhaWxzLlxuICAgKiBAcGFyYW0ge1JlbmRlclNlc3Npb259IHNlc3Npb24gLSBUaGUgVHlwc3QgZG9jdW1lbnQgc2Vzc2lvbiB0aGF0IGhhcyBiZWVuXG4gICAqIGNyZWF0ZWQgYnkgVHlwc3RSZW5kZXJlci5cbiAgICogQHBhcmFtIHtNYW5pcHVsYXRlRGF0YU9wdGlvbnN9IG9wdHMgLSBUaGUgb3B0aW9ucyBmb3IgbWFuaXB1bGF0aW5nIHRoZVxuICAgKiBUeXBzdCBkb2N1bWVudCBpbiB0aGUgc2Vzc2lvbi5cbiAgICpcbiAgICogQGV4YW1wbGVcbiAgICogcmVzZXQgdGhlIGRhdGEgdG8gdGhlIGluaXRpYWwgc3RhdGUuXG4gICAqIGBgYHR5cGVzY3JpcHRcbiAgICogY29uc3Qgc2Vzc2lvbiA9IGF3YWl0IHJlbmRlcmVyLmNyZWF0ZVNlc3Npb24oLi4uKTtcbiAgICogYXdhaXQgcmVuZGVyZXIubWFuaXB1bGF0ZURhdGEoc2Vzc2lvbiwge1xuICAgKiAgIGFjdGlvbjogJ3Jlc2V0JyxcbiAgICogICBkYXRhOiBuZXcgVWludDhBcnJheSguLi4pLFxuICAgKiB9KTtcbiAgICogYGBgXG4gICAqIEBleGFtcGxlXG4gICAqIG1lcmdlIHRoZSBkYXRhIHRvIHRoZSBjdXJyZW50IHN0YXRlLlxuICAgKiBgYGB0eXBlc2NyaXB0XG4gICAqIGNvbnN0IHNlc3Npb24gPSBhd2FpdCByZW5kZXJlci5jcmVhdGVTZXNzaW9uKC4uLik7XG4gICAqIC8vLyByZXNldCB0aGUgZGF0YSB0byB0aGUgaW5pdGlhbCBzdGF0ZVxuICAgKiBhd2FpdCByZW5kZXJlci5tYW5pcHVsYXRlRGF0YShzZXNzaW9uLCBkYXRhKCdyZXNldCcpKTtcbiAgICogLy8vIG1lcmdlIHRoZSBkYXRhIHRvIHRoZSBjdXJyZW50IHN0YXRlXG4gICAqIGF3YWl0IHJlbmRlcmVyLm1hbmlwdWxhdGVEYXRhKHNlc3Npb24sIGRhdGEoJ21lcmdlJykpO1xuICAgKiAvLy8gaW5jcmVtZW50YWxseSBtZXJnZSB0aGUgZGF0YSBhZ2FpblxuICAgKiBhd2FpdCByZW5kZXJlci5tYW5pcHVsYXRlRGF0YShzZXNzaW9uLCBkYXRhKCdtZXJnZScpKTtcbiAgICogYGBgXG4gICAqL1xuICBtYW5pcHVsYXRlRGF0YShvcHRzOiBSZW5kZXJJblNlc3Npb25PcHRpb25zPE1hbmlwdWxhdGVEYXRhT3B0aW9ucz4pOiB2b2lkO1xuXG4gIC8qKlxuICAgKiBSdW4gYSBmdW5jdGlvbiB3aXRoIGEgc2Vzc2lvbiwgYW5kIHRoZSBzZXNzaW9uIGlzIG9ubHkgYXZhaWxhYmxlIGR1cmluZ1xuICAgKiB0aGUgZnVuY3Rpb24gY2FsbC5cbiAgICpcbiAgICogdGhlIGxpZmV0aW1lIG9mIHNlc3Npb24gaXMgcXVpdGUgYnVnLXByb25lLCBzbyB3ZSBjdXJyZW50IGRvZXMgbm90IG1ha2UgaXRcbiAgICogbG9uZ2VyIGxpdmUgdGhhbiB0aGUgZnVuY3Rpb24gY2FsbC5cbiAgICogQHBhcmFtIHtmdW5jdGlvbn0gZm4gLSBUaGUgZnVuY3Rpb24gdG8gcnVuIHdpdGggYSBzZXNzaW9uLlxuICAgKiBAcmV0dXJucyB7UHJvbWlzZTxUPn0gLSBUaGUgcmVzdWx0IG9mIHRoZSBmdW5jdGlvbi5cbiAgICogQGV4YW1wbGVcbiAgICogcnVuIGEgZnVuY3Rpb24gd2l0aCBhbiBzZXNzaW9uIHdpdGggZW1wdHkgc3RhdGUuXG4gICAqXG4gICAqIGBgYHR5cGVzY3JpcHRcbiAgICogY29uc3QgcmVzID0gYXdhaXQgcmVuZGVyZXIucnVuV2l0aFNlc3Npb24oYXN5bmMgc2Vzc2lvbiA9PiB7XG4gICAqICAgYXdhaXQgcmVuZGVyZXIubWFuaXB1bGF0ZURhdGEoc2Vzc2lvbiwgZGF0YSgncmVzZXQnKSk7XG4gICAqICAgcmV0dXJuIGF3YWl0IHJlbmRlcmVyLnJlbmRlclRvQ2FudmFzKHtcbiAgICogICAgIHJlbmRlclNlc3Npb246IHNlc3Npb24sXG4gICAqICAgICBjb250YWluZXI6IGRvY3VtZW50LmdldEVsZW1lbnRCeUlkKCdjb250YWluZXInKSxcbiAgICogICAgIGJhY2tncm91bmRDb2xvcjogJyNmZmZmZmYnLFxuICAgKiAgIH0pO1xuICAgKiB9KTtcbiAgICogYGBgXG4gICAqXG4gICAqIEBleGFtcGxlXG4gICAqIHJ1biBhIGZ1bmN0aW9uIHdpdGggYW4gc2Vzc2lvbiB3aXRoIGluaXRpYWwgc3RhdGUuXG4gICAqXG4gICAqIGBgYHR5cGVzY3JpcHRcbiAgICogY29uc3QgcmVzID0gYXdhaXQgcmVuZGVyZXIucnVuV2l0aFNlc3Npb24oe1xuICAgKiAgIGZvcm1hdDogJ3ZlY3RvcicsXG4gICAqICAgYXJ0aWZhY3RDb250ZW50OiBuZXcgVWludDhBcnJheSguLi4pLFxuICAgKiB9LCB3b3JrV2l0aFNlc3Npb24oc2Vzc2lvbikpO1xuICAgKiBgYGBcbiAgICpcbiAgICogQGV4YW1wbGVcbiAgICogbGVhayB0aGUgbGlmZSBzcGFuIG9mIHNlc3Npb24gKG5lZWQgdHlwZXNjcmlwdCA+PSB2NS4yKVxuICAgKlxuICAgKiBgYGB0eXBlc2NyaXB0XG4gICAqIGNsYXNzIFN0YWNrZWRTZXNzaW9uIHtcbiAgICogICBzZXNzaW9uOiBSZW5kZXJTZXNzaW9uO1xuICAgKiAgIHByaXZhdGUgcmVzb2x2ZTogKHNlc3Npb246IFJlbmRlclNlc3Npb24pID0+IHZvaWQ7XG4gICAqICAgW1N5bWJvbC5kaXNwb3NlXSgpIHtcbiAgICogICAgIHRoaXMucmVzb2x2ZSgpO1xuICAgKiAgIH1cbiAgICogICBzdGF0aWMgYXN5bmMgY3JlYXRlKCkge1xuICAgKiAgICAgcmV0dXJuIG5ldyBQcm9taXNlPFN0YWNrZWRTZXNzaW9uPihyZXNvbHZlID0+IHtcbiAgICogICAgICAgY29uc3Qgc2Vzc2lvbiA9IGF3YWl0IHJlbmRlcmVyLnJ1bldpdGhTZXNzaW9uKHNlc3Npb24gPT4ge1xuICAgKiAgICAgICBjb25zdCBzdGFja2VkU2Vzc2lvbiA9IG5ldyBTdGFja2VkU2Vzc2lvbigpO1xuICAgKiAgICAgICBzdGFja2VkU2Vzc2lvbi5zZXNzaW9uID0gc2Vzc2lvbjtcbiAgICogICAgICAgc3RhY2tlZFNlc3Npb24ucmVzb2x2ZSA9IHJlc29sdmU7XG4gICAqICAgICAgIHJldHVybiBzdGFja2VkU2Vzc2lvbjtcbiAgICogICAgIH0pO1xuICAgKiAgIH1cbiAgICogfVxuICAgKlxuICAgKiB7XG4gICAqICAgYXdhaXQgdXNpbmcgc2Vzc2lvbiA9IFN0YWNrZWRTZXNzaW9uLmNyZWF0ZSgpO1xuICAgKiAgIC8vLyBkbyBzb21ldGhpbmcgd2l0aCBzZXNzaW9uXG4gICAqIH1cbiAgICogYGBgXG4gICAqL1xuICBydW5XaXRoU2Vzc2lvbjxUPihmbjogKHNlc3Npb246IFJlbmRlclNlc3Npb24pID0+IFByb21pc2U8VD4pOiBQcm9taXNlPFQ+O1xuICBydW5XaXRoU2Vzc2lvbjxUPihcbiAgICBvcHRpb25zOiBDcmVhdGVTZXNzaW9uT3B0aW9ucyxcbiAgICBmbjogKHNlc3Npb246IFJlbmRlclNlc3Npb24pID0+IFByb21pc2U8VD4sXG4gICk6IFByb21pc2U8VD47XG5cbiAgY3JlYXRlV29ya2VyVjAod29ya2VyOiBXb3JrZXIpOiBQcm9taXNlPFR5cHN0V29ya2VyPjtcblxuICByZW5kZXJEb20ob3B0aW9uczogUmVuZGVySW5TZXNzaW9uT3B0aW9uczxNb3VudERvbU9wdGlvbnM+KTogUHJvbWlzZTxUeXBzdERvbURvY3VtZW50PjtcblxuICAvKipcbiAgICogYWxpYXMgdG8ge0BsaW5rIFR5cHN0UmVuZGVyZXIjcmVuZGVyVG9DYW52YXN9LCB3aWxsIHJlbW92ZSBpbiB2MC41LjBcbiAgICogQGRlcHJlY2F0ZWRcbiAgICogdXNlIHtAbGluayByZW5kZXJUb0NhbnZhc30gaW5zdGVhZFxuICAgKi9cbiAgcmVuZGVyKG9wdGlvbnM6IFJlbmRlck9wdGlvbnM8UmVuZGVyVG9DYW52YXNPcHRpb25zPik6IFByb21pc2U8dm9pZD47XG59XG5cbmNvbnN0IGdSZW5kZXJlck1vZHVsZSA9IChtb2R1bGU6IHR5cGVvZiB0eXBzdCkgPT5cbiAgbmV3IExhenlXYXNtTW9kdWxlKGFzeW5jIChiaW4/OiBhbnkpID0+IHtcbiAgICByZXR1cm4gYXdhaXQgbW9kdWxlLmRlZmF1bHQoYmluKTtcbiAgfSk7XG5cbi8qKlxuICogY3JlYXRlIGEgVHlwc3QgcmVuZGVyZXIuXG4gKiBAcmV0dXJucyB7VHlwc3RSZW5kZXJlcn0gLSBUaGUgVHlwc3QgcmVuZGVyZXIuXG4gKiBAZXhhbXBsZVxuICogYGBgdHlwZXNjcmlwdFxuICogaW1wb3J0IHsgY3JlYXRlVHlwc3RSZW5kZXJlciB9IGZyb20gJ3R5cHN0JztcbiAqIGNvbnN0IHJlbmRlcmVyID0gY3JlYXRlVHlwc3RSZW5kZXJlcigpO1xuICogYXdhaXQgcmVuZGVyZXIuaW5pdCgpO1xuICogYXdhaXQgcmVuZGVyZXIucmVuZGVyKHtcbiAqICAgY29udGFpbmVyOiBkb2N1bWVudC5nZXRFbGVtZW50QnlJZCgnY29udGFpbmVyJyksXG4gKiAgIGFydGlmYWN0Q29udGVudDogJ3sgLi4uIH0nLFxuICogfSk7XG4gKiBgYGBcbiAqL1xuZXhwb3J0IGZ1bmN0aW9uIGNyZWF0ZVR5cHN0UmVuZGVyZXIoKTogVHlwc3RSZW5kZXJlciB7XG4gIHJldHVybiBuZXcgVHlwc3RSZW5kZXJlckRyaXZlcigpO1xufVxuXG5leHBvcnQgYXN5bmMgZnVuY3Rpb24gcmVuZGVyZXJCdWlsZEluZm8oKTogUHJvbWlzZTxhbnk+IHtcbiAgY29uc3QgcmVuZGVyTW9kdWxlID0gYXdhaXQgaW1wb3J0KCdAbXlyaWFkZHJlYW1pbi90eXBzdC10cy1yZW5kZXJlcicpO1xuICByZXR1cm4gcmVuZGVyTW9kdWxlLnJlbmRlcmVyX2J1aWxkX2luZm8oKTtcbn1cblxubGV0IHdhcm5PbmNlQ2FudmFzU2V0ID0gdHJ1ZTtcblxuLyoqIEBpbnRlcm5hbCAqL1xuZXhwb3J0IGNsYXNzIFR5cHN0UmVuZGVyZXJEcml2ZXIge1xuICByZW5kZXJlcjogdHlwc3QuVHlwc3RSZW5kZXJlcjtcbiAgcmVuZGVyZXJKczogdHlwZW9mIHR5cHN0O1xuXG4gIGNvbnN0cnVjdG9yKCkgeyB9XG5cbiAgYXN5bmMgaW5pdChvcHRpb25zPzogUGFydGlhbDxJbml0T3B0aW9ucz4pOiBQcm9taXNlPHZvaWQ+IHtcbiAgICB0aGlzLnJlbmRlcmVySnMgPSBhd2FpdCAob3B0aW9ucz8uZ2V0V3JhcHBlcj8uKCkgfHwgaW1wb3J0KCdAbXlyaWFkZHJlYW1pbi90eXBzdC10cy1yZW5kZXJlcicpKTtcbiAgICBjb25zdCBUeXBzdFJlbmRlcmVyQnVpbGRlciA9IHRoaXMucmVuZGVyZXJKcy5UeXBzdFJlbmRlcmVyQnVpbGRlcjtcbiAgICB0aGlzLnJlbmRlcmVyID0gYXdhaXQgYnVpbGRDb21wb25lbnQoXG4gICAgICBvcHRpb25zLFxuICAgICAgZ1JlbmRlcmVyTW9kdWxlKHRoaXMucmVuZGVyZXJKcyksXG4gICAgICBUeXBzdFJlbmRlcmVyQnVpbGRlcixcbiAgICAgIHt9LFxuICAgICk7XG4gIH1cblxuICBsb2FkR2x5cGhQYWNrKF9wYWNrOiB1bmtub3duKTogUHJvbWlzZTx2b2lkPiB7XG4gICAgLy8gdGhpcy5yZW5kZXJlci5sb2FkX2dseXBoX3BhY2socGFjayk7XG4gICAgcmV0dXJuIFByb21pc2UucmVzb2x2ZSgpO1xuICB9XG5cbiAgcHJpdmF0ZSBjcmVhdGVPcHRpb25zVG9SdXN0KG9wdGlvbnM6IFBhcnRpYWw8Q3JlYXRlU2Vzc2lvbk9wdGlvbnM+KTogdHlwc3QuQ3JlYXRlU2Vzc2lvbk9wdGlvbnMge1xuICAgIGNvbnN0IHJ1c3RPcHRpb25zID0gbmV3IHRoaXMucmVuZGVyZXJKcy5DcmVhdGVTZXNzaW9uT3B0aW9ucygpO1xuXG4gICAgaWYgKG9wdGlvbnMuZm9ybWF0ICE9PSB1bmRlZmluZWQpIHtcbiAgICAgIHJ1c3RPcHRpb25zLmZvcm1hdCA9IG9wdGlvbnMuZm9ybWF0O1xuICAgIH1cblxuICAgIGlmIChvcHRpb25zLmFydGlmYWN0Q29udGVudCAhPT0gdW5kZWZpbmVkKSB7XG4gICAgICBydXN0T3B0aW9ucy5hcnRpZmFjdF9jb250ZW50ID0gb3B0aW9ucy5hcnRpZmFjdENvbnRlbnQ7XG4gICAgfVxuXG4gICAgcmV0dXJuIHJ1c3RPcHRpb25zO1xuICB9XG5cbiAgY2FudmFzT3B0aW9uc1RvUnVzdChvcHRpb25zOiBSZW5kZXJDYW52YXNPcHRpb25zKTogdHlwc3QuUmVuZGVyUGFnZUltYWdlT3B0aW9ucyB7XG4gICAgY29uc3QgcnVzdE9wdGlvbnMgPSBuZXcgdGhpcy5yZW5kZXJlckpzLlJlbmRlclBhZ2VJbWFnZU9wdGlvbnMoKTtcbiAgICBpZiAob3B0aW9ucy5wYWdlT2Zmc2V0ID09PSB1bmRlZmluZWQpIHtcbiAgICAgIHRocm93IG5ldyBFcnJvcigncGFnZU9mZnNldCBpcyByZXF1aXJlZCBpbiByZWZsZXhvIHYwLjUuMCcpO1xuICAgIH0gZWxzZSB7XG4gICAgICBydXN0T3B0aW9ucy5wYWdlX29mZiA9IG9wdGlvbnMucGFnZU9mZnNldDtcbiAgICB9XG4gICAgaWYgKG9wdGlvbnMuY2FjaGVLZXkgIT09IHVuZGVmaW5lZCkge1xuICAgICAgcnVzdE9wdGlvbnMuY2FjaGVfa2V5ID0gb3B0aW9ucy5jYWNoZUtleTtcbiAgICB9XG4gICAgaWYgKG9wdGlvbnMuYmFja2dyb3VuZENvbG9yICE9PSB1bmRlZmluZWQpIHtcbiAgICAgIHJ1c3RPcHRpb25zLmJhY2tncm91bmRfY29sb3IgPSBvcHRpb25zLmJhY2tncm91bmRDb2xvcjtcbiAgICB9XG4gICAgaWYgKG9wdGlvbnMucGl4ZWxQZXJQdCAhPT0gdW5kZWZpbmVkKSB7XG4gICAgICBydXN0T3B0aW9ucy5waXhlbF9wZXJfcHQgPSBvcHRpb25zLnBpeGVsUGVyUHQ7XG4gICAgfVxuICAgIGlmIChvcHRpb25zLmRhdGFTZWxlY3Rpb24gIT09IHVuZGVmaW5lZCkge1xuICAgICAgbGV0IGVuY29kZWQgPSAwO1xuICAgICAgaWYgKG9wdGlvbnMuZGF0YVNlbGVjdGlvbi5ib2R5KSB7XG4gICAgICAgIGVuY29kZWQgfD0gMSA8PCAwO1xuICAgICAgfSBlbHNlIGlmIChvcHRpb25zLmNhbnZhcyAmJiB3YXJuT25jZUNhbnZhc1NldCkge1xuICAgICAgICB3YXJuT25jZUNhbnZhc1NldCA9IGZhbHNlO1xuICAgICAgICBjb25zb2xlLndhcm4oJ2RhdGFTZWxlY3Rpb24uYm9keSBpcyBub3Qgc2V0IGJ1dCBwcm92aWRpbmcgY2FudmFzIGZvciBib2R5Jyk7XG4gICAgICB9XG4gICAgICBpZiAob3B0aW9ucy5kYXRhU2VsZWN0aW9uLnRleHQgfHwgb3B0aW9ucy5kYXRhU2VsZWN0aW9uLmFubm90YXRpb24pIHtcbiAgICAgICAgY29uc29sZS5lcnJvcignZGF0YVNlbGVjdGlvbi50ZXh0IGFuZCBkYXRhU2VsZWN0aW9uLmFubm90YXRpb24gYXJlIGRlcHJlY2F0ZWQnKTtcbiAgICAgIH1cbiAgICAgIGlmIChvcHRpb25zLmRhdGFTZWxlY3Rpb24uc2VtYW50aWNzKSB7XG4gICAgICAgIGVuY29kZWQgfD0gMSA8PCAzO1xuICAgICAgfVxuICAgICAgcnVzdE9wdGlvbnMuZGF0YV9zZWxlY3Rpb24gPSBlbmNvZGVkO1xuICAgIH1cbiAgICByZXR1cm4gcnVzdE9wdGlvbnM7XG4gIH1cblxuICByZXRyaWV2ZVBhZ2VzSW5mb0Zyb21TZXNzaW9uKHNlc3Npb246IFJlbmRlclNlc3Npb24pOiBQYWdlSW5mb1tdIHtcbiAgICByZXR1cm4gc2Vzc2lvbi5yZXRyaWV2ZVBhZ2VzSW5mbygpO1xuICB9XG5cbiAgLyoqXG4gICAqIFJlbmRlciBhIFR5cHN0IGRvY3VtZW50IHRvIGNhbnZhcy5cbiAgICovXG4gIHJlbmRlckNhbnZhcyhvcHRpb25zOiBSZW5kZXJPcHRpb25zPFJlbmRlckNhbnZhc09wdGlvbnM+KTogUHJvbWlzZTxSZW5kZXJDYW52YXNSZXN1bHQ+IHtcbiAgICByZXR1cm4gdGhpcy53aXRoaW5PcHRpb25TZXNzaW9uKG9wdGlvbnMsIGFzeW5jIHNlc3Npb25SZWYgPT4ge1xuICAgICAgcmV0dXJuIHRoaXMucmVuZGVyZXIucmVuZGVyX3BhZ2VfdG9fY2FudmFzKFxuICAgICAgICBzZXNzaW9uUmVmW2tPYmplY3RdLFxuICAgICAgICBvcHRpb25zLmNhbnZhcyB8fCB1bmRlZmluZWQsXG4gICAgICAgIHRoaXMuY2FudmFzT3B0aW9uc1RvUnVzdChvcHRpb25zKSxcbiAgICAgICk7XG4gICAgfSk7XG4gIH1cblxuICAvLyBhc3luYyByZW5kZXJQZGYoYXJ0aWZhY3RDb250ZW50OiBzdHJpbmcpOiBQcm9taXNlPFVpbnQ4QXJyYXk+IHtcbiAgLy8gcmV0dXJuIHRoaXMucmVuZGVyZXIucmVuZGVyX3RvX3BkZihhcnRpZmFjdENvbnRlbnQpO1xuICAvLyB9XG5cbiAgcHJpdmF0ZSBhc3luYyBpbkFuaW1hdGlvbkZyYW1lPFQ+KGZuOiAoKSA9PiBQcm9taXNlPFQ+KTogUHJvbWlzZTxUPiB7XG4gICAgcmV0dXJuIG5ldyBQcm9taXNlKChyZXNvbHZlLCByZWplY3QpID0+IHtcbiAgICAgIHJlcXVlc3RBbmltYXRpb25GcmFtZSgoKSA9PiB7XG4gICAgICAgIHRyeSB7XG4gICAgICAgICAgcmVzb2x2ZShmbigpKTtcbiAgICAgICAgfSBjYXRjaCAoZSkge1xuICAgICAgICAgIHJlamVjdChlKTtcbiAgICAgICAgfVxuICAgICAgfSk7XG4gICAgfSk7XG4gIH1cblxuICBwcml2YXRlIGFzeW5jIHJlbmRlckRpc3BsYXlMYXllcihcbiAgICBzZXNzaW9uOiBSZW5kZXJTZXNzaW9uLFxuICAgIGNhbnZhc0xpc3Q6IEhUTUxDYW52YXNFbGVtZW50W10sXG4gICAgb3B0aW9uczogUmVuZGVyVG9DYW52YXNPcHRpb25zLFxuICApOiBQcm9taXNlPFJlbmRlckNhbnZhc1Jlc3VsdFtdPiB7XG4gICAgY29uc3QgcGFnZXNfaW5mbyA9IHNlc3Npb25ba09iamVjdF0ucGFnZXNfaW5mbztcbiAgICBjb25zdCBwYWdlX2NvdW50ID0gcGFnZXNfaW5mby5wYWdlX2NvdW50O1xuXG4gICAgY29uc3QgZG9SZW5kZXIgPSBhc3luYyAoaTogbnVtYmVyLCBwYWdlX29mZjogbnVtYmVyKSA9PiB7XG4gICAgICBjb25zdCBjYW52YXMgPSBjYW52YXNMaXN0W2ldO1xuICAgICAgY29uc3QgY3R4ID0gY2FudmFzLmdldENvbnRleHQoJzJkJyk7XG4gICAgICBpZiAoIWN0eCkge1xuICAgICAgICB0aHJvdyBuZXcgRXJyb3IoJ2NhbnZhcyBjb250ZXh0IGlzIG51bGwnKTtcbiAgICAgIH1cbiAgICAgIHJldHVybiBhd2FpdCB0aGlzLnJlbmRlckNhbnZhcyh7XG4gICAgICAgIC4uLm9wdGlvbnMsXG4gICAgICAgIGNhbnZhczogY3R4LFxuICAgICAgICByZW5kZXJTZXNzaW9uOiBzZXNzaW9uLFxuICAgICAgICBwYWdlT2Zmc2V0OiBwYWdlX29mZixcbiAgICAgIH0pO1xuICAgIH07XG5cbiAgICBjb25zdCB0ID0gcGVyZm9ybWFuY2Uubm93KCk7XG4gICAgY29uc3QgdGV4dENvbnRlbnRMaXN0ID0gYXdhaXQgKGFzeW5jICgpID0+IHtcbiAgICAgIGNvbnN0IHJlc3VsdHM6IFJlbmRlckNhbnZhc1Jlc3VsdFtdID0gW107XG4gICAgICBmb3IgKGxldCBpID0gMDsgaSA8IHBhZ2VfY291bnQ7IGkrKykge1xuICAgICAgICByZXN1bHRzLnB1c2goYXdhaXQgdGhpcy5pbkFuaW1hdGlvbkZyYW1lKCgpID0+IGRvUmVuZGVyKGksIGkpKSk7XG4gICAgICB9XG5cbiAgICAgIHJldHVybiByZXN1bHRzO1xuICAgIH0pKCk7XG4gICAgY29uc3QgdDIgPSBwZXJmb3JtYW5jZS5ub3coKTtcblxuICAgIGNvbnNvbGUubG9nKGBkaXNwbGF5IGxheWVyIHVzZWQ6IHJlbmRlciA9ICR7KHQyIC0gdCkudG9GaXhlZCgxKX1tc2ApO1xuXG4gICAgcmV0dXJuIHRleHRDb250ZW50TGlzdDtcbiAgfVxuXG4gIHByaXZhdGUgcmVuZGVyVGV4dExheWVyKGxheWVyTGlzdDogSFRNTERpdkVsZW1lbnRbXSwgdGV4dFNvdXJjZUxpc3Q6IFJlbmRlckNhbnZhc1Jlc3VsdFtdKSB7XG4gICAgY29uc3QgdDIgPSBwZXJmb3JtYW5jZS5ub3coKTtcbiAgICBsYXllckxpc3QuZm9yRWFjaCgobGF5ZXIsIGkpID0+IHtcbiAgICAgIGxheWVyLmlubmVySFRNTCA9IHRleHRTb3VyY2VMaXN0W2ldLmh0bWxTZW1hbnRpY3NbMF07XG4gICAgfSk7XG4gICAgY29uc3QgdDMgPSBwZXJmb3JtYW5jZS5ub3coKTtcbiAgICBjb25zb2xlLmxvZyhgdGV4dCBsYXllciB1c2VkOiByZW5kZXIgPSAkeyh0MyAtIHQyKS50b0ZpeGVkKDEpfW1zYCk7XG4gIH1cblxuICBhc3luYyByZW5kZXIob3B0aW9uczogUmVuZGVyT3B0aW9uczxSZW5kZXJUb0NhbnZhc09wdGlvbnM+KTogUHJvbWlzZTx2b2lkPiB7XG4gICAgaWYgKCdmb3JtYXQnIGluIG9wdGlvbnMpIHtcbiAgICAgIGlmIChvcHRpb25zLmZvcm1hdCAhPT0gJ3ZlY3RvcicpIHtcbiAgICAgICAgY29uc3QgYXJ0aWZhY3RGb3JtYXRzID0gWydzZXJkZV9qc29uJywgJ2pzJywgJ2lyJ10gYXMgY29uc3Q7XG4gICAgICAgIGlmIChhcnRpZmFjdEZvcm1hdHMuaW5jbHVkZXMob3B0aW9ucy5mb3JtYXQgYXMgYW55KSkge1xuICAgICAgICAgIC8vIGRlcHJlY2F0ZWRcbiAgICAgICAgICB0aHJvdyBuZXcgRXJyb3IoYGRlcHJlY2F0ZWQgZm9ybWF0ICR7b3B0aW9ucy5mb3JtYXR9LCBwbGVhc2UgdXNlIHZlY3RvciBmb3JtYXRgKTtcbiAgICAgICAgfVxuICAgICAgfVxuICAgIH1cblxuICAgIHJldHVybiB0aGlzLnJlbmRlclRvQ2FudmFzKG9wdGlvbnMpO1xuICB9XG5cbiAgYXN5bmMgcmVuZGVyRG9tKG9wdGlvbnM6IFJlbmRlckluU2Vzc2lvbk9wdGlvbnM8TW91bnREb21PcHRpb25zPik6IFByb21pc2U8VHlwc3REb21Eb2N1bWVudD4ge1xuICAgIGlmICgnZm9ybWF0JyBpbiBvcHRpb25zKSB7XG4gICAgICBpZiAob3B0aW9ucy5mb3JtYXQgIT09ICd2ZWN0b3InKSB7XG4gICAgICAgIGNvbnN0IGFydGlmYWN0Rm9ybWF0cyA9IFsnc2VyZGVfanNvbicsICdqcycsICdpciddIGFzIGNvbnN0O1xuICAgICAgICBpZiAoYXJ0aWZhY3RGb3JtYXRzLmluY2x1ZGVzKG9wdGlvbnMuZm9ybWF0IGFzIGFueSkpIHtcbiAgICAgICAgICAvLyBkZXByZWNhdGVkXG4gICAgICAgICAgdGhyb3cgbmV3IEVycm9yKGBkZXByZWNhdGVkIGZvcm1hdCAke29wdGlvbnMuZm9ybWF0fSwgcGxlYXNlIHVzZSB2ZWN0b3IgZm9ybWF0YCk7XG4gICAgICAgIH1cbiAgICAgIH1cbiAgICB9XG5cbiAgICByZXR1cm4gdGhpcy53aXRoaW5PcHRpb25TZXNzaW9uKG9wdGlvbnMsIGFzeW5jIHNlc3Npb25SZWYgPT4ge1xuICAgICAgY29uc3QgdCA9IG5ldyBUeXBzdERvbURvY3VtZW50KHtcbiAgICAgICAgLi4ub3B0aW9ucyxcbiAgICAgICAgcmVuZGVyTW9kZTogJ2RvbScsXG4gICAgICAgIGhvb2tlZEVsZW06IG9wdGlvbnMuY29udGFpbmVyLFxuICAgICAgICBrTW9kdWxlOiBzZXNzaW9uUmVmLFxuICAgICAgICByZW5kZXJlcjogdGhpcyxcbiAgICAgIH0pO1xuICAgICAgdDtcbiAgICAgIGF3YWl0IHQuaW1wbC5tb3VudERvbShvcHRpb25zLnBpeGVsUGVyUHQpO1xuICAgICAgcmV0dXJuIHQ7XG4gICAgfSk7XG4gIH1cblxuICBhc3luYyByZW5kZXJUb0NhbnZhcyhvcHRpb25zOiBSZW5kZXJPcHRpb25zPFJlbmRlclRvQ2FudmFzT3B0aW9ucz4pOiBQcm9taXNlPHZvaWQ+IHtcbiAgICBsZXQgc2Vzc2lvbjogUmVuZGVyU2Vzc2lvbjtcbiAgICBsZXQgcmVuZGVyUGFnZVJlc3VsdHM6IFJlbmRlckNhbnZhc1Jlc3VsdFtdO1xuICAgIGNvbnN0IG1vdW50Q29udGFpbmVyID0gb3B0aW9ucy5jb250YWluZXI7XG4gICAgbW91bnRDb250YWluZXIuc3R5bGUudmlzaWJpbGl0eSA9ICdoaWRkZW4nO1xuXG4gICAgY29uc3QgZG9SZW5kZXJEaXNwbGF5TGF5ZXIgPSBhc3luYyAoXG4gICAgICBjYW52YXNMaXN0OiBIVE1MQ2FudmFzRWxlbWVudFtdLFxuICAgICAgcmVzZXRMYXlvdXQ6ICgpID0+IHZvaWQsXG4gICAgKSA9PiB7XG4gICAgICB0cnkge1xuICAgICAgICByZW5kZXJQYWdlUmVzdWx0cyA9IGF3YWl0IHRoaXMucmVuZGVyRGlzcGxheUxheWVyKHNlc3Npb24sIGNhbnZhc0xpc3QsIG9wdGlvbnMpO1xuICAgICAgICByZXNldExheW91dCgpO1xuICAgICAgfSBmaW5hbGx5IHtcbiAgICAgICAgbW91bnRDb250YWluZXIuc3R5bGUudmlzaWJpbGl0eSA9ICd2aXNpYmxlJztcbiAgICAgIH1cbiAgICB9O1xuXG4gICAgcmV0dXJuIHRoaXMud2l0aGluT3B0aW9uU2Vzc2lvbihvcHRpb25zLCBhc3luYyBzZXNzaW9uUmVmID0+IHtcbiAgICAgIHNlc3Npb24gPSBzZXNzaW9uUmVmO1xuICAgICAgaWYgKHNlc3Npb25ba09iamVjdF0ucGFnZXNfaW5mby5wYWdlX2NvdW50ID09PSAwKSB7XG4gICAgICAgIHRocm93IG5ldyBFcnJvcihgTm8gcGFnZSBmb3VuZCBpbiBzZXNzaW9uYCk7XG4gICAgICB9XG5cbiAgICAgIGlmIChvcHRpb25zLnBpeGVsUGVyUHQgIT09IHVuZGVmaW5lZCAmJiBvcHRpb25zLnBpeGVsUGVyUHQgPD0gMCkge1xuICAgICAgICB0aHJvdyBuZXcgRXJyb3IoXG4gICAgICAgICAgJ0ludmFsaWQgdHlwc3QuUmVuZGVyT3B0aW9ucy5waXhlbFBlclB0LCBzaG91bGQgYmUgYSBwb3NpdGl2ZSBudW1iZXIgJyArXG4gICAgICAgICAgb3B0aW9ucy5waXhlbFBlclB0LFxuICAgICAgICApO1xuICAgICAgfVxuXG4gICAgICBsZXQgYmFja2dyb3VuZENvbG9yID0gb3B0aW9ucy5iYWNrZ3JvdW5kQ29sb3I7XG4gICAgICBpZiAoYmFja2dyb3VuZENvbG9yICE9PSB1bmRlZmluZWQpIHtcbiAgICAgICAgaWYgKCEvXiNbMC05YS1mXXs2fSQvLnRlc3QoYmFja2dyb3VuZENvbG9yKSkge1xuICAgICAgICAgIHRocm93IG5ldyBFcnJvcihcbiAgICAgICAgICAgICdJbnZhbGlkIHR5cHN0LmJhY2tncm91bmRDb2xvciBjb2xvciBmb3IgbWF0Y2hpbmcgXiM/WzAtOWEtZl17Nn0kICcgKyBiYWNrZ3JvdW5kQ29sb3IsXG4gICAgICAgICAgKTtcbiAgICAgICAgfVxuICAgICAgfVxuXG4gICAgICBzZXNzaW9uLnBpeGVsUGVyUHQgPSBvcHRpb25zLnBpeGVsUGVyUHQgPz8gVHlwc3REZWZhdWx0UGFyYW1zLlBJWEVMX1BFUl9QVDtcbiAgICAgIHNlc3Npb24uYmFja2dyb3VuZENvbG9yID0gYmFja2dyb3VuZENvbG9yID8/ICcjZmZmZmZmJztcblxuICAgICAgY29uc3QgdCA9IHBlcmZvcm1hbmNlLm5vdygpO1xuXG4gICAgICBjb25zdCBwYWdlVmlldyA9IG5ldyBSZW5kZXJWaWV3KFxuICAgICAgICB0aGlzLnJldHJpZXZlUGFnZXNJbmZvRnJvbVNlc3Npb24oc2Vzc2lvbiksXG4gICAgICAgIG1vdW50Q29udGFpbmVyLFxuICAgICAgICBvcHRpb25zLFxuICAgICAgKTtcbiAgICAgIGNvbnN0IHQyID0gcGVyZm9ybWFuY2Uubm93KCk7XG5cbiAgICAgIGNvbnNvbGUubG9nKGBsYXllciB1c2VkOiByZXRyaWV2ZSA9ICR7KHQyIC0gdCkudG9GaXhlZCgxKX1tc2ApO1xuXG4gICAgICBhd2FpdCBkb1JlbmRlckRpc3BsYXlMYXllcihwYWdlVmlldy5jYW52YXNMaXN0LCAoKSA9PiBwYWdlVmlldy5yZXNldExheW91dCgpKTtcbiAgICAgIHRoaXMucmVuZGVyVGV4dExheWVyKHBhZ2VWaWV3LnRleHRMYXllckxpc3QsIHJlbmRlclBhZ2VSZXN1bHRzKTtcblxuICAgICAgcmV0dXJuO1xuICAgIH0pO1xuICB9XG5cbiAgY3JlYXRlTW9kdWxlKGI/OiBVaW50OEFycmF5KTogUHJvbWlzZTxSZW5kZXJTZXNzaW9uPiB7XG4gICAgcmV0dXJuIFByb21pc2UucmVzb2x2ZShcbiAgICAgIG5ldyBSZW5kZXJTZXNzaW9uKFxuICAgICAgICB0aGlzLFxuICAgICAgICB0aGlzLnJlbmRlcmVyLmNyZWF0ZV9zZXNzaW9uKFxuICAgICAgICAgIGIgJiZcbiAgICAgICAgICB0aGlzLmNyZWF0ZU9wdGlvbnNUb1J1c3Qoe1xuICAgICAgICAgICAgZm9ybWF0OiAndmVjdG9yJyxcbiAgICAgICAgICAgIGFydGlmYWN0Q29udGVudDogYixcbiAgICAgICAgICB9KSxcbiAgICAgICAgKSxcbiAgICAgICksXG4gICAgKTtcbiAgfVxuXG4gIGFzeW5jIGNyZWF0ZVdvcmtlclYwKHdvcmtlcjogV29ya2VyKSB7XG4gICAgcmV0dXJuIG5ldyBUeXBzdFdvcmtlcih0aGlzLCBhd2FpdCB0aGlzLnJlbmRlcmVyLmNyZWF0ZV93b3JrZXIod29ya2VyKSk7XG4gIH1cblxuICB3b3JrZXJCcmlkZ2UoKSB7XG4gICAgcmV0dXJuIHRoaXMucmVuZGVyZXIuY3JlYXRlX3dvcmtlcl9icmlkZ2UoKTtcbiAgfVxuXG4gIHJlbmRlclN2ZyhvcHRpb25zOiBSZW5kZXJPcHRpb25zPFJlbmRlclN2Z09wdGlvbnM+LCBjb250YWluZXI/OiBhbnkpOiBQcm9taXNlPHN0cmluZz4ge1xuICAgIGlmIChvcHRpb25zIGluc3RhbmNlb2YgUmVuZGVyU2Vzc2lvbiB8fCBjb250YWluZXIpIHtcbiAgICAgIHRocm93IG5ldyBFcnJvcigncmVtb3ZlZCBhcGksIHBsZWFzZSB1c2UgcmVuZGVyVG9TdmcoeyByZW5kZXJTZXNzaW9uLCBjb250YWluZXIgfSkgaW5zdGVhZCcpO1xuICAgIH1cblxuICAgIHJldHVybiB0aGlzLndpdGhpbk9wdGlvblNlc3Npb24ob3B0aW9ucywgYXN5bmMgc2Vzc2lvblJlZiA9PiB7XG4gICAgICBsZXQgcGFydHM6IG51bWJlciB8IHVuZGVmaW5lZCA9IHVuZGVmaW5lZDtcbiAgICAgIGlmIChvcHRpb25zLmRhdGFfc2VsZWN0aW9uKSB7XG4gICAgICAgIHBhcnRzID0gMDtcbiAgICAgICAgaWYgKG9wdGlvbnMuZGF0YV9zZWxlY3Rpb24uYm9keSkge1xuICAgICAgICAgIHBhcnRzIHw9IDEgPDwgMDtcbiAgICAgICAgfVxuICAgICAgICBpZiAob3B0aW9ucy5kYXRhX3NlbGVjdGlvbi5kZWZzKSB7XG4gICAgICAgICAgcGFydHMgfD0gMSA8PCAxO1xuICAgICAgICB9XG4gICAgICAgIGlmIChvcHRpb25zLmRhdGFfc2VsZWN0aW9uLmNzcykge1xuICAgICAgICAgIHBhcnRzIHw9IDEgPDwgMjtcbiAgICAgICAgfVxuICAgICAgICBpZiAob3B0aW9ucy5kYXRhX3NlbGVjdGlvbi5qcykge1xuICAgICAgICAgIHBhcnRzIHw9IDEgPDwgMztcbiAgICAgICAgfVxuICAgICAgfVxuXG4gICAgICByZXR1cm4gUHJvbWlzZS5yZXNvbHZlKHRoaXMucmVuZGVyZXIuc3ZnX2RhdGEoc2Vzc2lvblJlZltrT2JqZWN0XSwgcGFydHMpKTtcbiAgICB9KTtcbiAgfVxuXG4gIHJlbmRlclN2Z0RpZmYob3B0aW9uczogUmVuZGVySW5TZXNzaW9uT3B0aW9uczxSZW5kZXJTdmdPcHRpb25zPik6IHN0cmluZyB7XG4gICAgaWYgKCFvcHRpb25zLndpbmRvdykge1xuICAgICAgcmV0dXJuIHRoaXMucmVuZGVyZXIucmVuZGVyX3N2Z19kaWZmKFxuICAgICAgICAob3B0aW9ucy5yZW5kZXJTZXNzaW9uIGFzIGFueSlba09iamVjdF0sXG4gICAgICAgIDAsXG4gICAgICAgIDAsXG4gICAgICAgIDFlMzMsXG4gICAgICAgIDFlMzMsXG4gICAgICApO1xuICAgIH1cblxuICAgIHJldHVybiB0aGlzLnJlbmRlcmVyLnJlbmRlcl9zdmdfZGlmZihcbiAgICAgIChvcHRpb25zLnJlbmRlclNlc3Npb24gYXMgYW55KVtrT2JqZWN0XSxcbiAgICAgIG9wdGlvbnMud2luZG93LmxvLngsXG4gICAgICBvcHRpb25zLndpbmRvdy5sby55LFxuICAgICAgb3B0aW9ucy53aW5kb3cuaGkueCxcbiAgICAgIG9wdGlvbnMud2luZG93LmhpLnksXG4gICAgKTtcbiAgfVxuXG4gIHJlbmRlclRvU3ZnKG9wdGlvbnM6IFJlbmRlck9wdGlvbnM8UmVuZGVyVG9TdmdPcHRpb25zPik6IFByb21pc2U8Ym9vbGVhbj4ge1xuICAgIHJldHVybiB0aGlzLndpdGhpbk9wdGlvblNlc3Npb24ob3B0aW9ucywgYXN5bmMgc2Vzc2lvblJlZiA9PiB7XG4gICAgICByZXR1cm4gUHJvbWlzZS5yZXNvbHZlKHRoaXMucmVuZGVyZXIucmVuZGVyX3N2ZyhzZXNzaW9uUmVmW2tPYmplY3RdLCBvcHRpb25zLmNvbnRhaW5lcikpO1xuICAgIH0pO1xuICB9XG5cbiAgZ2V0Q3VzdG9tVjEob3B0aW9uczogUmVuZGVySW5TZXNzaW9uT3B0aW9uczx7fT4pOiBQcm9taXNlPGFueT4ge1xuICAgIHJldHVybiBQcm9taXNlLnJlc29sdmUodGhpcy5yZW5kZXJlci5nZXRfY3VzdG9tcyhvcHRpb25zLnJlbmRlclNlc3Npb25ba09iamVjdF0pKTtcbiAgfVxuXG4gIHJlc2V0U2Vzc2lvbihzZXNzaW9uOiBSZW5kZXJTZXNzaW9uKTogdm9pZCB7XG4gICAgcmV0dXJuIHRoaXMucmVuZGVyZXIucmVzZXQoc2Vzc2lvbltrT2JqZWN0XSk7XG4gIH1cblxuICBtYW5pcHVsYXRlRGF0YShvcHRzOiBSZW5kZXJJblNlc3Npb25PcHRpb25zPE1hbmlwdWxhdGVEYXRhT3B0aW9ucz4pOiB2b2lkIHtcbiAgICByZXR1cm4gdGhpcy5yZW5kZXJlci5tYW5pcHVsYXRlX2RhdGEoXG4gICAgICAob3B0cy5yZW5kZXJTZXNzaW9uIGFzIGFueSlba09iamVjdF0gYXMgdHlwc3QuUmVuZGVyU2Vzc2lvbixcbiAgICAgIG9wdHMuYWN0aW9uID8/ICdyZXNldCcsXG4gICAgICBvcHRzLmRhdGEsXG4gICAgKTtcbiAgfVxuXG4gIHByaXZhdGUgd2l0aGluT3B0aW9uU2Vzc2lvbjxUPihcbiAgICBvcHRpb25zOiBSZW5kZXJPcHRpb25zPGFueT4sXG4gICAgZm46IChzZXNzaW9uOiBSZW5kZXJTZXNzaW9uKSA9PiBQcm9taXNlPFQ+LFxuICApOiBQcm9taXNlPFQ+IHtcbiAgICBmdW5jdGlvbiBpc1JlbmRlckJ5Q29udGVudE9wdGlvbihvcHRpb25zOiBSZW5kZXJPcHRpb25zPGFueT4pOiBvcHRpb25zIGlzIENyZWF0ZVNlc3Npb25PcHRpb25zIHtcbiAgICAgIHJldHVybiAnYXJ0aWZhY3RDb250ZW50JyBpbiBvcHRpb25zO1xuICAgIH1cblxuICAgIGlmICgncmVuZGVyU2Vzc2lvbicgaW4gb3B0aW9ucykge1xuICAgICAgcmV0dXJuIGZuKG9wdGlvbnMucmVuZGVyU2Vzc2lvbiBhcyBSZW5kZXJTZXNzaW9uKTtcbiAgICB9XG5cbiAgICBpZiAoaXNSZW5kZXJCeUNvbnRlbnRPcHRpb24ob3B0aW9ucykpIHtcbiAgICAgIC8vIHRvZG86IHJlbW92ZSBhbnlcbiAgICAgIHJldHVybiB0aGlzLnJ1bldpdGhTZXNzaW9uKG9wdGlvbnMgYXMgYW55LCBmbiBhcyBhbnkpO1xuICAgIH1cblxuICAgIHRocm93IG5ldyBFcnJvcihcbiAgICAgICdJbnZhbGlkIHJlbmRlciBvcHRpb25zLCBzaG91bGQgYmUgb25lIG9mIFJlbmRlckJ5Q29udGVudE9wdGlvbnN8UmVuZGVyQnlTZXNzaW9uT3B0aW9ucycsXG4gICAgKTtcbiAgfVxuXG4gIGFzeW5jIHJ1bldpdGhTZXNzaW9uPFQ+KGZuOiAoc2Vzc2lvbjogUmVuZGVyU2Vzc2lvbikgPT4gUHJvbWlzZTxUPik6IFByb21pc2U8VD47XG4gIHJ1bldpdGhTZXNzaW9uPFQ+KFxuICAgIG9wdGlvbnM6IENyZWF0ZVNlc3Npb25PcHRpb25zLFxuICAgIGZuOiAoc2Vzc2lvbjogUmVuZGVyU2Vzc2lvbikgPT4gUHJvbWlzZTxUPixcbiAgKTogUHJvbWlzZTxUPjtcbiAgYXN5bmMgcnVuV2l0aFNlc3Npb248VD4oYXJnMTogYW55LCBhcmcyPzogYW55KTogUHJvbWlzZTxUPiB7XG4gICAgbGV0IG9wdGlvbnM6IFBhcnRpYWw8Q3JlYXRlU2Vzc2lvbk9wdGlvbnM+IHwgdW5kZWZpbmVkID0gYXJnMTtcbiAgICBsZXQgZm46IChzZXNzaW9uOiBSZW5kZXJTZXNzaW9uKSA9PiBQcm9taXNlPFQ+ID0gYXJnMjtcblxuICAgIGlmICghYXJnMikge1xuICAgICAgb3B0aW9ucyA9IHVuZGVmaW5lZDtcbiAgICAgIGZuID0gYXJnMTtcbiAgICB9XG5cbiAgICBjb25zdCBzZXNzaW9uID0gdGhpcy5yZW5kZXJlci5jcmVhdGVfc2Vzc2lvbihcbiAgICAgIC8qIG1vdmVkICovIG9wdGlvbnMgJiYgdGhpcy5jcmVhdGVPcHRpb25zVG9SdXN0KG9wdGlvbnMpLFxuICAgICk7XG4gICAgdHJ5IHtcbiAgICAgIGNvbnN0IHJlcyA9IGF3YWl0IGZuKG5ldyBSZW5kZXJTZXNzaW9uKHRoaXMsIHNlc3Npb24pKTtcbiAgICAgIHNlc3Npb24uZnJlZSgpO1xuICAgICAgcmV0dXJuIHJlcztcbiAgICB9IGNhdGNoIChlKSB7XG4gICAgICBzZXNzaW9uLmZyZWUoKTtcbiAgICAgIHRocm93IGU7XG4gICAgfVxuICB9XG59XG4iXX0=