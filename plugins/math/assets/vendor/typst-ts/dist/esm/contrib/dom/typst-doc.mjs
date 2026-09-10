export var PreviewMode;
(function (PreviewMode) {
    PreviewMode[PreviewMode["Doc"] = 0] = "Doc";
    PreviewMode[PreviewMode["Slide"] = 1] = "Slide";
})(PreviewMode || (PreviewMode = {}));
export class TypstDocumentContext {
    hookedElem;
    kModule;
    opts;
    modes = [];
    /// Configuration fields
    /// enable partial rendering
    partialRendering = true;
    /// underlying renderer
    renderMode = 'svg';
    r = undefined;
    /// preview mode
    previewMode = PreviewMode.Doc;
    /// whether this is a content preview
    isContentPreview = false;
    /// whether this content preview will mix outline titles
    isMixinOutline = false;
    /// background color
    backgroundColor = 'black';
    /// default page color (empty string means transparent)
    pageColor = 'white';
    /// pixel per pt
    pixelPerPt = 3;
    /// customized way to retrieving dom state
    retrieveDOMState;
    /// State fields
    /// whether svg is updating (in triggerSvgUpdate)
    isRendering = false;
    /// whether kModule is initialized
    moduleInitialized = false;
    /// patch queue for updating data.
    patchQueue = [];
    /// resources to dispose
    disposeList = [];
    /// canvas render ctoken
    canvasRenderCToken;
    /// There are two scales in this class: The real scale is to adjust the size
    /// of `hookedElem` to fit the svg. The virtual scale (scale ratio) is to let
    /// user zoom in/out the svg. For example:
    /// + the default value of virtual scale is 1, which means the svg is totally
    ///   fit in `hookedElem`.
    /// + if user set virtual scale to 0.5, then the svg will be zoomed out to fit
    ///   in half width of `hookedElem`. "real" current scale of `hookedElem`
    currentRealScale = 1;
    /// "virtual" current scale of `hookedElem`
    currentScaleRatio = 1;
    /// timeout for delayed viewport change
    vpTimeout = undefined;
    /// sampled by last render time.
    sampledRenderTime = 0;
    /// page to partial render
    partialRenderPage = 0;
    /// outline data
    outline = undefined;
    /// cursor position in form of [page, x, y]
    cursorPosition = undefined;
    // id: number = rnd++;
    /// Cache fields
    /// cached state of container, default to retrieve state from `this.hookedElem`
    cachedDOMState = {
        width: 0,
        height: 0,
        window: {
            innerWidth: 0,
            innerHeight: 0,
        },
        boundingRect: {
            left: 0,
            top: 0,
            right: 0,
        },
    };
    constructor(opts) {
        this.hookedElem = opts.hookedElem;
        this.kModule = opts.kModule;
        this.opts = opts || {};
        /// Apply configuration
        {
            const { renderMode, previewMode, isContentPreview, retrieveDOMState } = opts || {};
            this.partialRendering = false;
            this.renderMode = renderMode ?? this.renderMode;
            this.previewMode = previewMode ?? this.previewMode;
            this.isContentPreview = isContentPreview || false;
            this.retrieveDOMState =
                retrieveDOMState ??
                    (() => {
                        return {
                            width: this.hookedElem.offsetWidth,
                            height: this.hookedElem.offsetHeight,
                            window: {
                                innerWidth: window.innerWidth,
                                innerHeight: window.innerHeight,
                            },
                            boundingRect: this.hookedElem.getBoundingClientRect(),
                        };
                    });
            this.backgroundColor = getComputedStyle(document.documentElement).getPropertyValue('--typst-preview-background-color');
        }
        // if init scale == 1
        // hide scrollbar if scale == 1
        this.hookedElem.classList.add('hide-scrollbar-x');
        this.hookedElem.parentElement?.classList.add('hide-scrollbar-x');
        if (this.previewMode === PreviewMode.Slide) {
            this.hookedElem.classList.add('hide-scrollbar-y');
            this.hookedElem.parentElement?.classList.add('hide-scrollbar-y');
        }
        this.installCtrlWheelHandler();
    }
    reset() {
        this.kModule.reset();
        this.moduleInitialized = false;
    }
    dispose() {
        const disposeList = this.disposeList;
        this.disposeList = [];
        disposeList.forEach(x => x());
    }
    static derive(ctx, mode) {
        return ['rescale', 'rerender', 'postRender'].reduce((acc, x) => {
            acc[x] = ctx[`${x}$${mode}`].bind(ctx);
            console.assert(acc[x] !== undefined, `${x}$${mode} is undefined`);
            return acc;
        }, {});
    }
    registerMode(mode) {
        const facade = TypstDocumentContext.derive(this, mode);
        this.modes.push([mode, facade]);
        if (mode === this.renderMode) {
            this.r = facade;
        }
    }
    installCtrlWheelHandler() {
        // Ctrl+scroll rescaling
        // will disable auto resizing
        // fixed factors, same as pdf.js
        const factors = [
            0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1, 1.1, 1.3, 1.5, 1.7, 1.9, 2.1, 2.4, 2.7, 3,
            3.3, 3.7, 4.1, 4.6, 5.1, 5.7, 6.3, 7, 7.7, 8.5, 9.4, 10,
        ];
        const wheelEventHandler = (event) => {
            if (event.ctrlKey) {
                event.preventDefault();
                // retrieve dom state before any operation
                this.cachedDOMState = this.retrieveDOMState();
                if (window.onresize !== null) {
                    // is auto resizing
                    window.onresize = null;
                }
                const prevScaleRatio = this.currentScaleRatio;
                // Get wheel scroll direction and calculate new scale
                if (event.deltaY < 0) {
                    // enlarge
                    if (this.currentScaleRatio >= factors.at(-1)) {
                        // already large than max factor
                        return;
                    }
                    else {
                        this.currentScaleRatio = factors.filter(x => x > this.currentScaleRatio).at(0);
                    }
                }
                else if (event.deltaY > 0) {
                    // reduce
                    if (this.currentScaleRatio <= factors.at(0)) {
                        return;
                    }
                    else {
                        this.currentScaleRatio = factors.filter(x => x < this.currentScaleRatio).at(-1);
                    }
                }
                else {
                    // no y-axis scroll
                    return;
                }
                const scrollFactor = this.currentScaleRatio / prevScaleRatio;
                const scrollX = event.pageX * (scrollFactor - 1);
                const scrollY = event.pageY * (scrollFactor - 1);
                // hide scrollbar if scale == 1
                if (Math.abs(this.currentScaleRatio - 1) < 1e-5) {
                    this.hookedElem.classList.add('hide-scrollbar-x');
                    this.hookedElem.parentElement?.classList.add('hide-scrollbar-x');
                    if (this.previewMode === PreviewMode.Slide) {
                        this.hookedElem.classList.add('hide-scrollbar-y');
                        this.hookedElem.parentElement?.classList.add('hide-scrollbar-y');
                    }
                }
                else {
                    this.hookedElem.classList.remove('hide-scrollbar-x');
                    this.hookedElem.parentElement?.classList.remove('hide-scrollbar-x');
                    if (this.previewMode === PreviewMode.Slide) {
                        this.hookedElem.classList.remove('hide-scrollbar-y');
                        this.hookedElem.parentElement?.classList.remove('hide-scrollbar-y');
                    }
                }
                // reserve space to scroll down
                const svg = this.hookedElem.firstElementChild;
                if (svg) {
                    const scaleRatio = this.getSvgScaleRatio();
                    const dataHeight = Number.parseFloat(svg.getAttribute('data-height'));
                    const scaledHeight = Math.ceil(dataHeight * scaleRatio);
                    // we increase the height by 2 times.
                    // The `2` is only a magic number that is large enough.
                    this.hookedElem.style.height = `${scaledHeight * 2}px`;
                }
                // make sure the cursor is still on the same position
                window.scrollBy(scrollX, scrollY);
                // toggle scale change event
                this.addViewportChange();
                return false;
            }
        };
        if (this.renderMode !== 'dom') {
            const vscodeAPI = typeof acquireVsCodeApi !== 'undefined';
            if (vscodeAPI) {
                window.addEventListener('wheel', wheelEventHandler, {
                    passive: false,
                });
                this.disposeList.push(() => {
                    window.removeEventListener('wheel', wheelEventHandler);
                });
            }
            else {
                document.body.addEventListener('wheel', wheelEventHandler, {
                    passive: false,
                });
                this.disposeList.push(() => {
                    document.body.removeEventListener('wheel', wheelEventHandler);
                });
            }
        }
    }
    /// Get current scale from html to svg
    // Note: one should retrieve dom state before rescale
    getSvgScaleRatio() {
        const svg = this.hookedElem.firstElementChild;
        if (!svg) {
            return 0;
        }
        const container = this.cachedDOMState;
        const svgWidth = Number.parseFloat(svg.getAttribute('data-width') || svg.getAttribute('width') || '1');
        const svgHeight = Number.parseFloat(svg.getAttribute('data-height') || svg.getAttribute('height') || '1');
        this.currentRealScale =
            this.previewMode === PreviewMode.Slide
                ? Math.min(container.width / svgWidth, container.height / svgHeight)
                : container.width / svgWidth;
        return this.currentRealScale * this.currentScaleRatio;
    }
    processQueue(svgUpdateEvent) {
        const eventName = svgUpdateEvent[0];
        switch (eventName) {
            case 'new':
            case 'diff-v1': {
                if (eventName === 'new') {
                    this.reset();
                }
                this.kModule.manipulateData({
                    action: 'merge',
                    data: svgUpdateEvent[1],
                });
                this.moduleInitialized = true;
                return true;
            }
            case 'viewport-change': {
                if (!this.moduleInitialized) {
                    console.log('viewport-change before initialization');
                    return false;
                }
                return true;
            }
            default:
                console.log('svgUpdateEvent', svgUpdateEvent);
                return false;
        }
    }
    triggerUpdate() {
        if (this.isRendering) {
            return;
        }
        this.isRendering = true;
        const doUpdate = async () => {
            this.cachedDOMState = this.retrieveDOMState();
            if (this.patchQueue.length === 0) {
                this.isRendering = false;
                this.postprocessChanges();
                return;
            }
            try {
                let t0 = performance.now();
                const ctoken = this.canvasRenderCToken;
                if (ctoken) {
                    await ctoken.cancel();
                    await ctoken.wait();
                    this.canvasRenderCToken = undefined;
                    console.log('cancel canvas rendering');
                }
                let needRerender = false;
                // console.log('patchQueue', JSON.stringify(this.patchQueue.map(x => x[0])));
                while (this.patchQueue.length > 0) {
                    needRerender = this.processQueue(this.patchQueue.shift()) || needRerender;
                }
                // todo: trigger viewport change once
                let t1 = performance.now();
                if (needRerender) {
                    this.r.rescale();
                    await this.r.rerender();
                    this.r.rescale();
                }
                let t2 = performance.now();
                /// perf event
                const d = (e, x, y) => `${e} ${(y - x).toFixed(2)} ms`;
                this.sampledRenderTime = t2 - t0;
                // todo: log in production
                // console.log([d('parse', t0, t1), d('rerender', t1, t2), d('total', t0, t2)].join(', '));
                requestAnimationFrame(doUpdate);
            }
            catch (e) {
                console.error(e);
                this.isRendering = false;
                this.postprocessChanges();
            }
        };
        requestAnimationFrame(doUpdate);
    }
    postprocessChanges() {
        this.r.postRender();
        // todo: abstract this
        if (this.previewMode === PreviewMode.Slide) {
            document.querySelectorAll('.typst-page-number-indicator').forEach(x => {
                x.textContent = `${this.kModule.retrievePagesInfo().length}`;
            });
        }
    }
    addChangement(change) {
        if (change[0] === 'new') {
            this.patchQueue.splice(0, this.patchQueue.length);
        }
        const pushChange = () => {
            this.vpTimeout = undefined;
            this.patchQueue.push(change);
            this.triggerUpdate();
        };
        if (this.vpTimeout !== undefined) {
            clearTimeout(this.vpTimeout);
        }
        if (change[0] === 'viewport-change' && this.isRendering) {
            // delay viewport change a bit
            this.vpTimeout = setTimeout(pushChange, this.sampledRenderTime || 100);
        }
        else {
            pushChange();
        }
    }
    addViewportChange() {
        this.addChangement(['viewport-change', '']);
    }
}
export function provideDoc(Base) {
    return class TypstDocument {
        impl;
        kModule;
        constructor(options) {
            if (options.isContentPreview) {
                options.renderMode = 'canvas';
            }
            this.kModule = options.kModule;
            this.impl = new Base(options);
            if (!this.impl.r) {
                throw new Error(`mode is not supported, ${options?.renderMode}`);
            }
            if (options.isContentPreview) {
                // content preview has very bad performance without partial rendering
                this.impl.partialRendering = true;
                this.impl.pixelPerPt = 1;
                this.impl.isMixinOutline = true;
            }
        }
        dispose() {
            this.impl.dispose();
        }
        reset() {
            this.impl.reset();
        }
        addChangement(change) {
            this.impl.addChangement(change);
        }
        addViewportChange() {
            this.impl.addViewportChange();
        }
        setPageColor(color) {
            this.impl.pageColor = color;
            this.addViewportChange();
        }
        setPartialRendering(partialRendering) {
            this.impl.partialRendering = partialRendering;
        }
        setCursor(page, x, y) {
            this.impl.cursorPosition = [page, x, y];
        }
        setPartialPageNumber(page) {
            if (page <= 0 || page > this.kModule.retrievePagesInfo().length) {
                return false;
            }
            this.impl.partialRenderPage = page - 1;
            this.addViewportChange();
            return true;
        }
        getPartialPageNumber() {
            return this.impl.partialRenderPage + 1;
        }
        setOutineData(outline) {
            this.impl.outline = outline;
            this.addViewportChange();
        }
    };
}
export function composeDoc(Base, ...mixins) {
    return mixins.reduce((acc, mixin) => mixin(acc), Base);
}
//# sourceMappingURL=data:application/json;base64,eyJ2ZXJzaW9uIjozLCJmaWxlIjoidHlwc3QtZG9jLm1qcyIsInNvdXJjZVJvb3QiOiIiLCJzb3VyY2VzIjpbIi4uLy4uLy4uLy4uL3NyYy9jb250cmliL2RvbS90eXBzdC1kb2MubXRzIl0sIm5hbWVzIjpbXSwibWFwcGluZ3MiOiJBQXVCQSxNQUFNLENBQU4sSUFBWSxXQUdYO0FBSEQsV0FBWSxXQUFXO0lBQ3JCLDJDQUFHLENBQUE7SUFDSCwrQ0FBSyxDQUFBO0FBQ1AsQ0FBQyxFQUhXLFdBQVcsS0FBWCxXQUFXLFFBR3RCO0FBb0JELE1BQU0sT0FBTyxvQkFBb0I7SUFDeEIsVUFBVSxDQUFjO0lBQ3hCLE9BQU8sQ0FBZ0I7SUFDdkIsSUFBSSxDQUFJO0lBQ2YsS0FBSyxHQUFvQyxFQUFFLENBQUM7SUFFNUMsd0JBQXdCO0lBRXhCLDRCQUE0QjtJQUM1QixnQkFBZ0IsR0FBWSxJQUFJLENBQUM7SUFDakMsdUJBQXVCO0lBQ3ZCLFVBQVUsR0FBZSxLQUFLLENBQUM7SUFDL0IsQ0FBQyxHQUF3QixTQUFVLENBQUM7SUFDcEMsZ0JBQWdCO0lBQ2hCLFdBQVcsR0FBZ0IsV0FBVyxDQUFDLEdBQUcsQ0FBQztJQUMzQyxxQ0FBcUM7SUFDckMsZ0JBQWdCLEdBQVksS0FBSyxDQUFDO0lBQ2xDLHdEQUF3RDtJQUN4RCxjQUFjLEdBQVksS0FBSyxDQUFDO0lBQ2hDLG9CQUFvQjtJQUNwQixlQUFlLEdBQVcsT0FBTyxDQUFDO0lBQ2xDLHVEQUF1RDtJQUN2RCxTQUFTLEdBQVcsT0FBTyxDQUFDO0lBQzVCLGdCQUFnQjtJQUNoQixVQUFVLEdBQVcsQ0FBQyxDQUFDO0lBQ3ZCLDBDQUEwQztJQUMxQyxnQkFBZ0IsQ0FBMEI7SUFFMUMsZ0JBQWdCO0lBRWhCLGlEQUFpRDtJQUNqRCxXQUFXLEdBQVksS0FBSyxDQUFDO0lBQzdCLGtDQUFrQztJQUNsQyxpQkFBaUIsR0FBWSxLQUFLLENBQUM7SUFDbkMsa0NBQWtDO0lBQ2xDLFVBQVUsR0FBdUIsRUFBRSxDQUFDO0lBQ3BDLHdCQUF3QjtJQUN4QixXQUFXLEdBQW1C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QUFFLENBQVM7WUFDMUMsSUFBSSxDQUFDLElBQUksQ0FBQyxjQUFjLEdBQUcsQ0FBQyxJQUFJLEVBQUUsQ0FBQyxFQUFFLENBQUMsQ0FBQyxDQUFDO1FBQzFDLENBQUM7UUFFRCxvQkFBb0IsQ0FBQyxJQUFZO1lBQy9CLElBQUksSUFBSSxJQUFJLENBQUMsSUFBSSxJQUFJLEdBQUcsSUFBSSxDQUFDLE9BQU8sQ0FBQyxpQkFBaUIsRUFBRSxDQUFDLE1BQU0sRUFBRSxDQUFDO2dCQUNoRSxPQUFPLEtBQUssQ0FBQztZQUNmLENBQUM7WUFDRCxJQUFJLENBQUMsSUFBSSxDQUFDLGlCQUFpQixHQUFHLElBQUksR0FBRyxDQUFDLENBQUM7WUFDdkMsSUFBSSxDQUFDLGlCQUFpQixFQUFFLENBQUM7WUFDekIsT0FBTyxJQUFJLENBQUM7UUFDZCxDQUFDO1FBRUQsb0JBQW9CO1lBQ2xCLE9BQU8sSUFBSSxDQUFDLElBQUksQ0FBQyxpQkFBaUIsR0FBRyxDQUFDLENBQUM7UUFDekMsQ0FBQztRQUVELGFBQWEsQ0FBQyxPQUFZO1lBQ3hCLElBQUksQ0FBQyxJQUFJLENBQUMsT0FBTyxHQUFHLE9BQU8sQ0FBQztZQUM1QixJQUFJLENBQUMsaUJBQWlCLEVBQUUsQ0FBQztRQUMzQixDQUFDO0tBQ0YsQ0FBQztBQUNKLENBQUM7QUEwRUQsTUFBTSxVQUFVLFVBQVUsQ0FBNkIsSUFBVyxFQUFFLEdBQUcsTUFBYTtJQUNsRixPQUFPLE1BQU0sQ0FBQyxNQUFNLENBQUMsQ0FBQyxHQUFHLEVBQUUsS0FBSyxFQUFFLEVBQUUsQ0FBQyxLQUFLLENBQUMsR0FBRyxDQUFDLEVBQUUsSUFBSSxDQUFDLENBQUM7QUFDekQsQ0FBQyIsInNvdXJjZXNDb250ZW50IjpbImltcG9ydCB0eXBlIHsgUmVuZGVyU2Vzc2lvbiB9IGZyb20gJy4uLy4uL3JlbmRlcmVyLm1qcyc7XG5pbXBvcnQgeyBUeXBzdENhbmNlbGxhdGlvblRva2VuIH0gZnJvbSAnLi90eXBzdC1jYW5jZWwubWpzJztcblxuZXhwb3J0IGludGVyZmFjZSBDb250YWluZXJET01TdGF0ZSB7XG4gIC8vLyBjYWNoZWQgYGhvb2tlZEVsZW0ub2Zmc2V0V2lkdGhgIG9yIGBob29rZWRFbGVtLmlubmVyV2lkdGhgXG4gIHdpZHRoOiBudW1iZXI7XG4gIC8vLyBjYWNoZWQgYGhvb2tlZEVsZW0ub2Zmc2V0SGVpZ2h0YCBvciBgaG9va2VkRWxlbS5pbm5lckhlaWdodGBcbiAgaGVpZ2h0OiBudW1iZXI7XG4gIHdpbmRvdzoge1xuICAgIGlubmVyV2lkdGg6IG51bWJlcjtcbiAgICBpbm5lckhlaWdodDogbnVtYmVyO1xuICB9O1xuICAvLy8gY2FjaGVkIGBob29rZWRFbGVtLmdldEJvdW5kaW5nQ2xpZW50UmVjdCgpYFxuICAvLy8gV2Ugb25seSB1c2UgYGxlZnRgIGFuZCBgdG9wYCBoZXJlLlxuICBib3VuZGluZ1JlY3Q6IHtcbiAgICBsZWZ0OiBudW1iZXI7XG4gICAgdG9wOiBudW1iZXI7XG4gICAgcmlnaHQ6IG51bWJlcjtcbiAgfTtcbn1cblxuZXhwb3J0IHR5cGUgUmVuZGVyTW9kZSA9ICdzdmcnIHwgJ2NhbnZhcycgfCAnZG9tJztcblxuZXhwb3J0IGVudW0gUHJldmlld01vZGUge1xuICBEb2MsXG4gIFNsaWRlLFxufVxuXG5leHBvcnQgaW50ZXJmYWNlIE9wdGlvbnMge1xuICBob29rZWRFbGVtOiBIVE1MRWxlbWVudDtcbiAga01vZHVsZTogUmVuZGVyU2Vzc2lvbjtcbiAgcmVuZGVyTW9kZT86IFJlbmRlck1vZGU7XG4gIHByZXZpZXdNb2RlPzogUHJldmlld01vZGU7XG4gIGlzQ29udGVudFByZXZpZXc/OiBib29sZWFuO1xuICBzb3VyY2VNYXBwaW5nPzogYm9vbGVhbjtcbiAgcmV0cmlldmVET01TdGF0ZT86ICgpID0+IENvbnRhaW5lckRPTVN0YXRlO1xufVxuXG5leHBvcnQgdHlwZSBHQ29uc3RydWN0b3I8VCA9IHt9PiA9IG5ldyAoLi4uYXJnczogYW55W10pID0+IFQ7XG5cbmludGVyZmFjZSBUeXBzdERvY3VtZW50RmFjYWRlIHtcbiAgcmVzY2FsZSgpOiB2b2lkO1xuICByZXJlbmRlcigpOiBQcm9taXNlPHZvaWQ+O1xuICBwb3N0UmVuZGVyKCk6IHZvaWQ7XG59XG5cbmV4cG9ydCBjbGFzcyBUeXBzdERvY3VtZW50Q29udGV4dDxPID0gYW55PiB7XG4gIHB1YmxpYyBob29rZWRFbGVtOiBIVE1MRWxlbWVudDtcbiAgcHVibGljIGtNb2R1bGU6IFJlbmRlclNlc3Npb247XG4gIHB1YmxpYyBvcHRzOiBPO1xuICBtb2RlczogW3N0cmluZywgVHlwc3REb2N1bWVudEZhY2FkZV1bXSA9IFtdO1xuXG4gIC8vLyBDb25maWd1cmF0aW9uIGZpZWxkc1xuXG4gIC8vLyBlbmFibGUgcGFydGlhbCByZW5kZXJpbmdcbiAgcGFydGlhbFJlbmRlcmluZzogYm9vbGVhbiA9IHRydWU7XG4gIC8vLyB1bmRlcmx5aW5nIHJlbmRlcmVyXG4gIHJlbmRlck1vZGU6IFJlbmRlck1vZGUgPSAnc3ZnJztcbiAgcjogVHlwc3REb2N1bWVudEZhY2FkZSA9IHVuZGVmaW5lZCE7XG4gIC8vLyBwcmV2aWV3IG1vZGVcbiAgcHJldmlld01vZGU6IFByZXZpZXdNb2RlID0gUHJldmlld01vZGUuRG9jO1xuICAvLy8gd2hldGhlciB0aGlzIGlzIGEgY29udGVudCBwcmV2aWV3XG4gIGlzQ29udGVudFByZXZpZXc6IGJvb2xlYW4gPSBmYWxzZTtcbiAgLy8vIHdoZXRoZXIgdGhpcyBjb250ZW50IHByZXZpZXcgd2lsbCBtaXggb3V0bGluZSB0aXRsZXNcbiAgaXNNaXhpbk91dGxpbmU6IGJvb2xlYW4gPSBmYWxzZTtcbiAgLy8vIGJhY2tncm91bmQgY29sb3JcbiAgYmFja2dyb3VuZENvbG9yOiBzdHJpbmcgPSAnYmxhY2snO1xuICAvLy8gZGVmYXVsdCBwYWdlIGNvbG9yIChlbXB0eSBzdHJpbmcgbWVhbnMgdHJhbnNwYXJlbnQpXG4gIHBhZ2VDb2xvcjogc3RyaW5nID0gJ3doaXRlJztcbiAgLy8vIHBpeGVsIHBlciBwdFxuICBwaXhlbFBlclB0OiBudW1iZXIgPSAzO1xuICAvLy8gY3VzdG9taXplZCB3YXkgdG8gcmV0cmlldmluZyBkb20gc3RhdGVcbiAgcmV0cmlldmVET01TdGF0ZTogKCkgPT4gQ29udGFpbmVyRE9NU3RhdGU7XG5cbiAgLy8vIFN0YXRlIGZpZWxkc1xuXG4gIC8vLyB3aGV0aGVyIHN2ZyBpcyB1cGRhdGluZyAoaW4gdHJpZ2dlclN2Z1VwZGF0ZSlcbiAgaXNSZW5kZXJpbmc6IGJvb2xlYW4gPSBmYWxzZTtcbiAgLy8vIHdoZXRoZXIga01vZHVsZSBpcyBpbml0aWFsaXplZFxuICBtb2R1bGVJbml0aWFsaXplZDogYm9vbGVhbiA9IGZhbHNlO1xuICAvLy8gcGF0Y2ggcXVldWUgZm9yIHVwZGF0aW5nIGRhdGEuXG4gIHBhdGNoUXVldWU6IFtzdHJpbmcsIHN0cmluZ11bXSA9IFtdO1xuICAvLy8gcmVzb3VyY2VzIHRvIGRpc3Bvc2VcbiAgZGlzcG9zZUxpc3Q6ICgoKSA9PiB2b2lkKVtdID0gW107XG4gIC8vLyBjYW52YXMgcmVuZGVyIGN0b2tlblxuICBjYW52YXNSZW5kZXJDVG9rZW4/OiBUeXBzdENhbmNlbGxhdGlvblRva2VuO1xuXG4gIC8vLyBUaGVyZSBhcmUgdHdvIHNjYWxlcyBpbiB0aGlzIGNsYXNzOiBUaGUgcmVhbCBzY2FsZSBpcyB0byBhZGp1c3QgdGhlIHNpemVcbiAgLy8vIG9mIGBob29rZWRFbGVtYCB0byBmaXQgdGhlIHN2Zy4gVGhlIHZpcnR1YWwgc2NhbGUgKHNjYWxlIHJhdGlvKSBpcyB0byBsZXRcbiAgLy8vIHVzZXIgem9vbSBpbi9vdXQgdGhlIHN2Zy4gRm9yIGV4YW1wbGU6XG4gIC8vLyArIHRoZSBkZWZhdWx0IHZhbHVlIG9mIHZpcnR1YWwgc2NhbGUgaXMgMSwgd2hpY2ggbWVhbnMgdGhlIHN2ZyBpcyB0b3RhbGx5XG4gIC8vLyAgIGZpdCBpbiBgaG9va2VkRWxlbWAuXG4gIC8vLyArIGlmIHVzZXIgc2V0IHZpcnR1YWwgc2NhbGUgdG8gMC41LCB0aGVuIHRoZSBzdmcgd2lsbCBiZSB6b29tZWQgb3V0IHRvIGZpdFxuICAvLy8gICBpbiBoYWxmIHdpZHRoIG9mIGBob29rZWRFbGVtYC4gXCJyZWFsXCIgY3VycmVudCBzY2FsZSBvZiBgaG9va2VkRWxlbWBcbiAgY3VycmVudFJlYWxTY2FsZTogbnVtYmVyID0gMTtcbiAgLy8vIFwidmlydHVhbFwiIGN1cnJlbnQgc2NhbGUgb2YgYGhvb2tlZEVsZW1gXG4gIGN1cnJlbnRTY2FsZVJhdGlvOiBudW1iZXIgPSAxO1xuICAvLy8gdGltZW91dCBmb3IgZGVsYXllZCB2aWV3cG9ydCBjaGFuZ2VcbiAgdnBUaW1lb3V0OiBhbnkgPSB1bmRlZmluZWQ7XG4gIC8vLyBzYW1wbGVkIGJ5IGxhc3QgcmVuZGVyIHRpbWUuXG4gIHNhbXBsZWRSZW5kZXJUaW1lOiBudW1iZXIgPSAwO1xuICAvLy8gcGFnZSB0byBwYXJ0aWFsIHJlbmRlclxuICBwYXJ0aWFsUmVuZGVyUGFnZTogbnVtYmVyID0gMDtcbiAgLy8vIG91dGxpbmUgZGF0YVxuICBvdXRsaW5lOiBhbnkgPSB1bmRlZmluZWQ7XG4gIC8vLyBjdXJzb3IgcG9zaXRpb24gaW4gZm9ybSBvZiBbcGFnZSwgeCwgeV1cbiAgY3Vyc29yUG9zaXRpb24/OiBbbnVtYmVyLCBudW1iZXIsIG51bWJlcl0gPSB1bmRlZmluZWQ7XG4gIC8vIGlkOiBudW1iZXIgPSBybmQrKztcblxuICAvLy8gQ2FjaGUgZmllbGRzXG5cbiAgLy8vIGNhY2hlZCBzdGF0ZSBvZiBjb250YWluZXIsIGRlZmF1bHQgdG8gcmV0cmlldmUgc3RhdGUgZnJvbSBgdGhpcy5ob29rZWRFbGVtYFxuICBjYWNoZWRET01TdGF0ZTogQ29udGFpbmVyRE9NU3RhdGUgPSB7XG4gICAgd2lkdGg6IDAsXG4gICAgaGVpZ2h0OiAwLFxuICAgIHdpbmRvdzoge1xuICAgICAgaW5uZXJXaWR0aDogMCxcbiAgICAgIGlubmVySGVpZ2h0OiAwLFxuICAgIH0sXG4gICAgYm91bmRpbmdSZWN0OiB7XG4gICAgICBsZWZ0OiAwLFxuICAgICAgdG9wOiAwLFxuICAgICAgcmlnaHQ6IDAsXG4gICAgfSxcbiAgfTtcblxuICBjb25zdHJ1Y3RvcihvcHRzOiBPcHRpb25zICYgTykge1xuICAgIHRoaXMuaG9va2VkRWxlbSA9IG9wdHMuaG9va2VkRWxlbTtcbiAgICB0aGlzLmtNb2R1bGUgPSBvcHRzLmtNb2R1bGU7XG4gICAgdGhpcy5vcHRzID0gb3B0cyB8fCB7fTtcblxuICAgIC8vLyBBcHBseSBjb25maWd1cmF0aW9uXG4gICAge1xuICAgICAgY29uc3QgeyByZW5kZXJNb2RlLCBwcmV2aWV3TW9kZSwgaXNDb250ZW50UHJldmlldywgcmV0cmlldmVET01TdGF0ZSB9ID0gb3B0cyB8fCB7fTtcbiAgICAgIHRoaXMucGFydGlhbFJlbmRlcmluZyA9IGZhbHNlO1xuICAgICAgdGhpcy5yZW5kZXJNb2RlID0gcmVuZGVyTW9kZSA/PyB0aGlzLnJlbmRlck1vZGU7XG4gICAgICB0aGlzLnByZXZpZXdNb2RlID0gcHJldmlld01vZGUgPz8gdGhpcy5wcmV2aWV3TW9kZTtcbiAgICAgIHRoaXMuaXNDb250ZW50UHJldmlldyA9IGlzQ29udGVudFByZXZpZXcgfHwgZmFsc2U7XG4gICAgICB0aGlzLnJldHJpZXZlRE9NU3RhdGUgPVxuICAgICAgICByZXRyaWV2ZURPTVN0YXRlID8/XG4gICAgICAgICgoKSA9PiB7XG4gICAgICAgICAgcmV0dXJuIHtcbiAgICAgICAgICAgIHdpZHRoOiB0aGlzLmhvb2tlZEVsZW0ub2Zmc2V0V2lkdGgsXG4gICAgICAgICAgICBoZWlnaHQ6IHRoaXMuaG9va2VkRWxlbS5vZmZzZXRIZWlnaHQsXG4gICAgICAgICAgICB3aW5kb3c6IHtcbiAgICAgICAgICAgICAgaW5uZXJXaWR0aDogd2luZG93LmlubmVyV2lkdGgsXG4gICAgICAgICAgICAgIGlubmVySGVpZ2h0OiB3aW5kb3cuaW5uZXJIZWlnaHQsXG4gICAgICAgICAgICB9LFxuICAgICAgICAgICAgYm91bmRpbmdSZWN0OiB0aGlzLmhvb2tlZEVsZW0uZ2V0Qm91bmRpbmdDbGllbnRSZWN0KCksXG4gICAgICAgICAgfTtcbiAgICAgICAgfSk7XG4gICAgICB0aGlzLmJhY2tncm91bmRDb2xvciA9IGdldENvbXB1dGVkU3R5bGUoZG9jdW1lbnQuZG9jdW1lbnRFbGVtZW50KS5nZXRQcm9wZXJ0eVZhbHVlKFxuICAgICAgICAnLS10eXBzdC1wcmV2aWV3LWJhY2tncm91bmQtY29sb3InLFxuICAgICAgKTtcbiAgICB9XG5cbiAgICAvLyBpZiBpbml0IHNjYWxlID09IDFcbiAgICAvLyBoaWRlIHNjcm9sbGJhciBpZiBzY2FsZSA9PSAxXG5cbiAgICB0aGlzLmhvb2tlZEVsZW0uY2xhc3NMaXN0LmFkZCgnaGlkZS1zY3JvbGxiYXIteCcpO1xuICAgIHRoaXMuaG9va2VkRWxlbS5wYXJlbnRFbGVtZW50Py5jbGFzc0xpc3QuYWRkKCdoaWRlLXNjcm9sbGJhci14Jyk7XG4gICAgaWYgKHRoaXMucHJldmlld01vZGUgPT09IFByZXZpZXdNb2RlLlNsaWRlKSB7XG4gICAgICB0aGlzLmhvb2tlZEVsZW0uY2xhc3NMaXN0LmFkZCgnaGlkZS1zY3JvbGxiYXIteScpO1xuICAgICAgdGhpcy5ob29rZWRFbGVtLnBhcmVudEVsZW1lbnQ/LmNsYXNzTGlzdC5hZGQoJ2hpZGUtc2Nyb2xsYmFyLXknKTtcbiAgICB9XG5cbiAgICB0aGlzLmluc3RhbGxDdHJsV2hlZWxIYW5kbGVyKCk7XG4gIH1cblxuICByZXNldCgpIHtcbiAgICB0aGlzLmtNb2R1bGUucmVzZXQoKTtcbiAgICB0aGlzLm1vZHVsZUluaXRpYWxpemVkID0gZmFsc2U7XG4gIH1cblxuICBkaXNwb3NlKCkge1xuICAgIGNvbnN0IGRpc3Bvc2VMaXN0ID0gdGhpcy5kaXNwb3NlTGlzdDtcbiAgICB0aGlzLmRpc3Bvc2VMaXN0ID0gW107XG4gICAgZGlzcG9zZUxpc3QuZm9yRWFjaCh4ID0+IHgoKSk7XG4gIH1cblxuICBzdGF0aWMgZGVyaXZlKGN0eDogYW55LCBtb2RlOiBzdHJpbmcpIHtcbiAgICByZXR1cm4gWydyZXNjYWxlJywgJ3JlcmVuZGVyJywgJ3Bvc3RSZW5kZXInXS5yZWR1Y2UoKGFjYzogYW55LCB4OiBzdHJpbmcpID0+IHtcbiAgICAgIGFjY1t4XSA9IGN0eFtgJHt4fSQke21vZGV9YF0uYmluZChjdHgpO1xuICAgICAgY29uc29sZS5hc3NlcnQoYWNjW3hdICE9PSB1bmRlZmluZWQsIGAke3h9JCR7bW9kZX0gaXMgdW5kZWZpbmVkYCk7XG4gICAgICByZXR1cm4gYWNjO1xuICAgIH0sIHt9IGFzIFR5cHN0RG9jdW1lbnRGYWNhZGUpO1xuICB9XG5cbiAgcmVnaXN0ZXJNb2RlKG1vZGU6IGFueSkge1xuICAgIGNvbnN0IGZhY2FkZSA9IFR5cHN0RG9jdW1lbnRDb250ZXh0LmRlcml2ZSh0aGlzLCBtb2RlKTtcbiAgICB0aGlzLm1vZGVzLnB1c2goW21vZGUsIGZhY2FkZV0pO1xuICAgIGlmIChtb2RlID09PSB0aGlzLnJlbmRlck1vZGUpIHtcbiAgICAgIHRoaXMuciA9IGZhY2FkZTtcbiAgICB9XG4gIH1cblxuICBwcml2YXRlIGluc3RhbGxDdHJsV2hlZWxIYW5kbGVyKCkge1xuICAgIC8vIEN0cmwrc2Nyb2xsIHJlc2NhbGluZ1xuICAgIC8vIHdpbGwgZGlzYWJsZSBhdXRvIHJlc2l6aW5nXG4gICAgLy8gZml4ZWQgZmFjdG9ycywgc2FtZSBhcyBwZGYuanNcbiAgICBjb25zdCBmYWN0b3JzID0gW1xuICAgICAgMC4xLCAwLjIsIDAuMywgMC40LCAwLjUsIDAuNiwgMC43LCAwLjgsIDAuOSwgMSwgMS4xLCAxLjMsIDEuNSwgMS43LCAxLjksIDIuMSwgMi40LCAyLjcsIDMsXG4gICAgICAzLjMsIDMuNywgNC4xLCA0LjYsIDUuMSwgNS43LCA2LjMsIDcsIDcuNywgOC41LCA5LjQsIDEwLFxuICAgIF07XG4gICAgY29uc3Qgd2hlZWxFdmVudEhhbmRsZXIgPSAoZXZlbnQ6IFdoZWVsRXZlbnQpID0+IHtcbiAgICAgIGlmIChldmVudC5jdHJsS2V5KSB7XG4gICAgICAgIGV2ZW50LnByZXZlbnREZWZhdWx0KCk7XG4gICAgICAgIC8vIHJldHJpZXZlIGRvbSBzdGF0ZSBiZWZvcmUgYW55IG9wZXJhdGlvblxuICAgICAgICB0aGlzLmNhY2hlZERPTVN0YXRlID0gdGhpcy5yZXRyaWV2ZURPTVN0YXRlKCk7XG4gICAgICAgIGlmICh3aW5kb3cub25yZXNpemUgIT09IG51bGwpIHtcbiAgICAgICAgICAvLyBpcyBhdXRvIHJlc2l6aW5nXG4gICAgICAgICAgd2luZG93Lm9ucmVzaXplID0gbnVsbDtcbiAgICAgICAgfVxuICAgICAgICBjb25zdCBwcmV2U2NhbGVSYXRpbyA9IHRoaXMuY3VycmVudFNjYWxlUmF0aW87XG4gICAgICAgIC8vIEdldCB3aGVlbCBzY3JvbGwgZGlyZWN0aW9uIGFuZCBjYWxjdWxhdGUgbmV3IHNjYWxlXG4gICAgICAgIGlmIChldmVudC5kZWx0YVkgPCAwKSB7XG4gICAgICAgICAgLy8gZW5sYXJnZVxuICAgICAgICAgIGlmICh0aGlzLmN1cnJlbnRTY2FsZVJhdGlvID49IGZhY3RvcnMuYXQoLTEpISkge1xuICAgICAgICAgICAgLy8gYWxyZWFkeSBsYXJnZSB0aGFuIG1heCBmYWN0b3JcbiAgICAgICAgICAgIHJldHVybjtcbiAgICAgICAgICB9IGVsc2Uge1xuICAgICAgICAgICAgdGhpcy5jdXJyZW50U2NhbGVSYXRpbyA9IGZhY3RvcnMuZmlsdGVyKHggPT4geCA+IHRoaXMuY3VycmVudFNjYWxlUmF0aW8pLmF0KDApITtcbiAgICAgICAgICB9XG4gICAgICAgIH0gZWxzZSBpZiAoZXZlbnQuZGVsdGFZID4gMCkge1xuICAgICAgICAgIC8vIHJlZHVjZVxuICAgICAgICAgIGlmICh0aGlzLmN1cnJlbnRTY2FsZVJhdGlvIDw9IGZhY3RvcnMuYXQoMCkhKSB7XG4gICAgICAgICAgICByZXR1cm47XG4gICAgICAgICAgfSBlbHNlIHtcbiAgICAgICAgICAgIHRoaXMuY3VycmVudFNjYWxlUmF0aW8gPSBmYWN0b3JzLmZpbHRlcih4ID0+IHggPCB0aGlzLmN1cnJlbnRTY2FsZVJhdGlvKS5hdCgtMSkhO1xuICAgICAgICAgIH1cbiAgICAgICAgfSBlbHNlIHtcbiAgICAgICAgICAvLyBubyB5LWF4aXMgc2Nyb2xsXG4gICAgICAgICAgcmV0dXJuO1xuICAgICAgICB9XG4gICAgICAgIGNvbnN0IHNjcm9sbEZhY3RvciA9IHRoaXMuY3VycmVudFNjYWxlUmF0aW8gLyBwcmV2U2NhbGVSYXRpbztcbiAgICAgICAgY29uc3Qgc2Nyb2xsWCA9IGV2ZW50LnBhZ2VYICogKHNjcm9sbEZhY3RvciAtIDEpO1xuICAgICAgICBjb25zdCBzY3JvbGxZID0gZXZlbnQucGFnZVkgKiAoc2Nyb2xsRmFjdG9yIC0gMSk7XG4gICAgICAgIC8vIGhpZGUgc2Nyb2xsYmFyIGlmIHNjYWxlID09IDFcbiAgICAgICAgaWYgKE1hdGguYWJzKHRoaXMuY3VycmVudFNjYWxlUmF0aW8gLSAxKSA8IDFlLTUpIHtcbiAgICAgICAgICB0aGlzLmhvb2tlZEVsZW0uY2xhc3NMaXN0LmFkZCgnaGlkZS1zY3JvbGxiYXIteCcpO1xuICAgICAgICAgIHRoaXMuaG9va2VkRWxlbS5wYXJlbnRFbGVtZW50Py5jbGFzc0xpc3QuYWRkKCdoaWRlLXNjcm9sbGJhci14Jyk7XG4gICAgICAgICAgaWYgKHRoaXMucHJldmlld01vZGUgPT09IFByZXZpZXdNb2RlLlNsaWRlKSB7XG4gICAgICAgICAgICB0aGlzLmhvb2tlZEVsZW0uY2xhc3NMaXN0LmFkZCgnaGlkZS1zY3JvbGxiYXIteScpO1xuICAgICAgICAgICAgdGhpcy5ob29rZWRFbGVtLnBhcmVudEVsZW1lbnQ/LmNsYXNzTGlzdC5hZGQoJ2hpZGUtc2Nyb2xsYmFyLXknKTtcbiAgICAgICAgICB9XG4gICAgICAgIH0gZWxzZSB7XG4gICAgICAgICAgdGhpcy5ob29rZWRFbGVtLmNsYXNzTGlzdC5yZW1vdmUoJ2hpZGUtc2Nyb2xsYmFyLXgnKTtcbiAgICAgICAgICB0aGlzLmhvb2tlZEVsZW0ucGFyZW50RWxlbWVudD8uY2xhc3NMaXN0LnJlbW92ZSgnaGlkZS1zY3JvbGxiYXIteCcpO1xuICAgICAgICAgIGlmICh0aGlzLnByZXZpZXdNb2RlID09PSBQcmV2aWV3TW9kZS5TbGlkZSkge1xuICAgICAgICAgICAgdGhpcy5ob29rZWRFbGVtLmNsYXNzTGlzdC5yZW1vdmUoJ2hpZGUtc2Nyb2xsYmFyLXknKTtcbiAgICAgICAgICAgIHRoaXMuaG9va2VkRWxlbS5wYXJlbnRFbGVtZW50Py5jbGFzc0xpc3QucmVtb3ZlKCdoaWRlLXNjcm9sbGJhci15Jyk7XG4gICAgICAgICAgfVxuICAgICAgICB9XG4gICAgICAgIC8vIHJlc2VydmUgc3BhY2UgdG8gc2Nyb2xsIGRvd25cbiAgICAgICAgY29uc3Qgc3ZnID0gdGhpcy5ob29rZWRFbGVtLmZpcnN0RWxlbWVudENoaWxkISBhcyBTVkdFbGVtZW50O1xuICAgICAgICBpZiAoc3ZnKSB7XG4gICAgICAgICAgY29uc3Qgc2NhbGVSYXRpbyA9IHRoaXMuZ2V0U3ZnU2NhbGVSYXRpbygpO1xuICAgICAgICAgIGNvbnN0IGRhdGFIZWlnaHQgPSBOdW1iZXIucGFyc2VGbG9hdChzdmcuZ2V0QXR0cmlidXRlKCdkYXRhLWhlaWdodCcpISk7XG4gICAgICAgICAgY29uc3Qgc2NhbGVkSGVpZ2h0ID0gTWF0aC5jZWlsKGRhdGFIZWlnaHQgKiBzY2FsZVJhdGlvKTtcbiAgICAgICAgICAvLyB3ZSBpbmNyZWFzZSB0aGUgaGVpZ2h0IGJ5IDIgdGltZXMuXG4gICAgICAgICAgLy8gVGhlIGAyYCBpcyBvbmx5IGEgbWFnaWMgbnVtYmVyIHRoYXQgaXMgbGFyZ2UgZW5vdWdoLlxuICAgICAgICAgIHRoaXMuaG9va2VkRWxlbS5zdHlsZS5oZWlnaHQgPSBgJHtzY2FsZWRIZWlnaHQgKiAyfXB4YDtcbiAgICAgICAgfVxuICAgICAgICAvLyBtYWtlIHN1cmUgdGhlIGN1cnNvciBpcyBzdGlsbCBvbiB0aGUgc2FtZSBwb3NpdGlvblxuICAgICAgICB3aW5kb3cuc2Nyb2xsQnkoc2Nyb2xsWCwgc2Nyb2xsWSk7XG4gICAgICAgIC8vIHRvZ2dsZSBzY2FsZSBjaGFuZ2UgZXZlbnRcbiAgICAgICAgdGhpcy5hZGRWaWV3cG9ydENoYW5nZSgpO1xuICAgICAgICByZXR1cm4gZmFsc2U7XG4gICAgICB9XG4gICAgfTtcblxuICAgIGlmICh0aGlzLnJlbmRlck1vZGUgIT09ICdkb20nKSB7XG4gICAgICBjb25zdCB2c2NvZGVBUEkgPSB0eXBlb2YgYWNxdWlyZVZzQ29kZUFwaSAhPT0gJ3VuZGVmaW5lZCc7XG4gICAgICBpZiAodnNjb2RlQVBJKSB7XG4gICAgICAgIHdpbmRvdy5hZGRFdmVudExpc3RlbmVyKCd3aGVlbCcsIHdoZWVsRXZlbnRIYW5kbGVyLCB7XG4gICAgICAgICAgcGFzc2l2ZTogZmFsc2UsXG4gICAgICAgIH0pO1xuICAgICAgICB0aGlzLmRpc3Bvc2VMaXN0LnB1c2goKCkgPT4ge1xuICAgICAgICAgIHdpbmRvdy5yZW1vdmVFdmVudExpc3RlbmVyKCd3aGVlbCcsIHdoZWVsRXZlbnRIYW5kbGVyKTtcbiAgICAgICAgfSk7XG4gICAgICB9IGVsc2Uge1xuICAgICAgICBkb2N1bWVudC5ib2R5LmFkZEV2ZW50TGlzdGVuZXIoJ3doZWVsJywgd2hlZWxFdmVudEhhbmRsZXIsIHtcbiAgICAgICAgICBwYXNzaXZlOiBmYWxzZSxcbiAgICAgICAgfSk7XG4gICAgICAgIHRoaXMuZGlzcG9zZUxpc3QucHVzaCgoKSA9PiB7XG4gICAgICAgICAgZG9jdW1lbnQuYm9keS5yZW1vdmVFdmVudExpc3RlbmVyKCd3aGVlbCcsIHdoZWVsRXZlbnRIYW5kbGVyKTtcbiAgICAgICAgfSk7XG4gICAgICB9XG4gICAgfVxuICB9XG5cbiAgLy8vIEdldCBjdXJyZW50IHNjYWxlIGZyb20gaHRtbCB0byBzdmdcbiAgLy8gTm90ZTogb25lIHNob3VsZCByZXRyaWV2ZSBkb20gc3RhdGUgYmVmb3JlIHJlc2NhbGVcbiAgZ2V0U3ZnU2NhbGVSYXRpbygpIHtcbiAgICBjb25zdCBzdmcgPSB0aGlzLmhvb2tlZEVsZW0uZmlyc3RFbGVtZW50Q2hpbGQgYXMgU1ZHRWxlbWVudDtcbiAgICBpZiAoIXN2Zykge1xuICAgICAgcmV0dXJuIDA7XG4gICAgfVxuXG4gICAgY29uc3QgY29udGFpbmVyID0gdGhpcy5jYWNoZWRET01TdGF0ZTtcblxuICAgIGNvbnN0IHN2Z1dpZHRoID0gTnVtYmVyLnBhcnNlRmxvYXQoXG4gICAgICBzdmcuZ2V0QXR0cmlidXRlKCdkYXRhLXdpZHRoJykgfHwgc3ZnLmdldEF0dHJpYnV0ZSgnd2lkdGgnKSB8fCAnMScsXG4gICAgKTtcbiAgICBjb25zdCBzdmdIZWlnaHQgPSBOdW1iZXIucGFyc2VGbG9hdChcbiAgICAgIHN2Zy5nZXRBdHRyaWJ1dGUoJ2RhdGEtaGVpZ2h0JykgfHwgc3ZnLmdldEF0dHJpYnV0ZSgnaGVpZ2h0JykgfHwgJzEnLFxuICAgICk7XG4gICAgdGhpcy5jdXJyZW50UmVhbFNjYWxlID1cbiAgICAgIHRoaXMucHJldmlld01vZGUgPT09IFByZXZpZXdNb2RlLlNsaWRlXG4gICAgICAgID8gTWF0aC5taW4oY29udGFpbmVyLndpZHRoIC8gc3ZnV2lkdGgsIGNvbnRhaW5lci5oZWlnaHQgLyBzdmdIZWlnaHQpXG4gICAgICAgIDogY29udGFpbmVyLndpZHRoIC8gc3ZnV2lkdGg7XG5cbiAgICByZXR1cm4gdGhpcy5jdXJyZW50UmVhbFNjYWxlICogdGhpcy5jdXJyZW50U2NhbGVSYXRpbztcbiAgfVxuXG4gIHByaXZhdGUgcHJvY2Vzc1F1ZXVlKHN2Z1VwZGF0ZUV2ZW50OiBbc3RyaW5nLCBzdHJpbmddKTogYm9vbGVhbiB7XG4gICAgY29uc3QgZXZlbnROYW1lID0gc3ZnVXBkYXRlRXZlbnRbMF07XG4gICAgc3dpdGNoIChldmVudE5hbWUpIHtcbiAgICAgIGNhc2UgJ25ldyc6XG4gICAgICBjYXNlICdkaWZmLXYxJzoge1xuICAgICAgICBpZiAoZXZlbnROYW1lID09PSAnbmV3Jykge1xuICAgICAgICAgIHRoaXMucmVzZXQoKTtcbiAgICAgICAgfVxuICAgICAgICB0aGlzLmtNb2R1bGUubWFuaXB1bGF0ZURhdGEoe1xuICAgICAgICAgIGFjdGlvbjogJ21lcmdlJyxcbiAgICAgICAgICBkYXRhOiBzdmdVcGRhdGVFdmVudFsxXSBhcyB1bmtub3duIGFzIFVpbnQ4QXJyYXksXG4gICAgICAgIH0pO1xuXG4gICAgICAgIHRoaXMubW9kdWxlSW5pdGlhbGl6ZWQgPSB0cnVlO1xuICAgICAgICByZXR1cm4gdHJ1ZTtcbiAgICAgIH1cbiAgICAgIGNhc2UgJ3ZpZXdwb3J0LWNoYW5nZSc6IHtcbiAgICAgICAgaWYgKCF0aGlzLm1vZHVsZUluaXRpYWxpemVkKSB7XG4gICAgICAgICAgY29uc29sZS5sb2coJ3ZpZXdwb3J0LWNoYW5nZSBiZWZvcmUgaW5pdGlhbGl6YXRpb24nKTtcbiAgICAgICAgICByZXR1cm4gZmFsc2U7XG4gICAgICAgIH1cbiAgICAgICAgcmV0dXJuIHRydWU7XG4gICAgICB9XG4gICAgICBkZWZhdWx0OlxuICAgICAgICBjb25zb2xlLmxvZygnc3ZnVXBkYXRlRXZlbnQnLCBzdmdVcGRhdGVFdmVudCk7XG4gICAgICAgIHJldHVybiBmYWxzZTtcbiAgICB9XG4gIH1cblxuICBwcml2YXRlIHRyaWdnZXJVcGRhdGUoKSB7XG4gICAgaWYgKHRoaXMuaXNSZW5kZXJpbmcpIHtcbiAgICAgIHJldHVybjtcbiAgICB9XG5cbiAgICB0aGlzLmlzUmVuZGVyaW5nID0gdHJ1ZTtcbiAgICBjb25zdCBkb1VwZGF0ZSA9IGFzeW5jICgpID0+IHtcbiAgICAgIHRoaXMuY2FjaGVkRE9NU3RhdGUgPSB0aGlzLnJldHJpZXZlRE9NU3RhdGUoKTtcblxuICAgICAgaWYgKHRoaXMucGF0Y2hRdWV1ZS5sZW5ndGggPT09IDApIHtcbiAgICAgICAgdGhpcy5pc1JlbmRlcmluZyA9IGZhbHNlO1xuICAgICAgICB0aGlzLnBvc3Rwcm9jZXNzQ2hhbmdlcygpO1xuICAgICAgICByZXR1cm47XG4gICAgICB9XG5cbiAgICAgIHRyeSB7XG4gICAgICAgIGxldCB0MCA9IHBlcmZvcm1hbmNlLm5vdygpO1xuXG4gICAgICAgIGNvbnN0IGN0b2tlbiA9IHRoaXMuY2FudmFzUmVuZGVyQ1Rva2VuO1xuICAgICAgICBpZiAoY3Rva2VuKSB7XG4gICAgICAgICAgYXdhaXQgY3Rva2VuLmNhbmNlbCgpO1xuICAgICAgICAgIGF3YWl0IGN0b2tlbi53YWl0KCk7XG4gICAgICAgICAgdGhpcy5jYW52YXNSZW5kZXJDVG9rZW4gPSB1bmRlZmluZWQ7XG4gICAgICAgICAgY29uc29sZS5sb2coJ2NhbmNlbCBjYW52YXMgcmVuZGVyaW5nJyk7XG4gICAgICAgIH1cblxuICAgICAgICBsZXQgbmVlZFJlcmVuZGVyID0gZmFsc2U7XG4gICAgICAgIC8vIGNvbnNvbGUubG9nKCdwYXRjaFF1ZXVlJywgSlNPTi5zdHJpbmdpZnkodGhpcy5wYXRjaFF1ZXVlLm1hcCh4ID0+IHhbMF0pKSk7XG4gICAgICAgIHdoaWxlICh0aGlzLnBhdGNoUXVldWUubGVuZ3RoID4gMCkge1xuICAgICAgICAgIG5lZWRSZXJlbmRlciA9IHRoaXMucHJvY2Vzc1F1ZXVlKHRoaXMucGF0Y2hRdWV1ZS5zaGlmdCgpISkgfHwgbmVlZFJlcmVuZGVyO1xuICAgICAgICB9XG5cbiAgICAgICAgLy8gdG9kbzogdHJpZ2dlciB2aWV3cG9ydCBjaGFuZ2Ugb25jZVxuICAgICAgICBsZXQgdDEgPSBwZXJmb3JtYW5jZS5ub3coKTtcbiAgICAgICAgaWYgKG5lZWRSZXJlbmRlcikge1xuICAgICAgICAgIHRoaXMuci5yZXNjYWxlKCk7XG4gICAgICAgICAgYXdhaXQgdGhpcy5yLnJlcmVuZGVyKCk7XG4gICAgICAgICAgdGhpcy5yLnJlc2NhbGUoKTtcbiAgICAgICAgfVxuICAgICAgICBsZXQgdDIgPSBwZXJmb3JtYW5jZS5ub3coKTtcblxuICAgICAgICAvLy8gcGVyZiBldmVudFxuICAgICAgICBjb25zdCBkID0gKGU6IHN0cmluZywgeDogbnVtYmVyLCB5OiBudW1iZXIpID0+IGAke2V9ICR7KHkgLSB4KS50b0ZpeGVkKDIpfSBtc2A7XG4gICAgICAgIHRoaXMuc2FtcGxlZFJlbmRlclRpbWUgPSB0MiAtIHQwO1xuICAgICAgICAvLyB0b2RvOiBsb2cgaW4gcHJvZHVjdGlvblxuICAgICAgICAvLyBjb25zb2xlLmxvZyhbZCgncGFyc2UnLCB0MCwgdDEpLCBkKCdyZXJlbmRlcicsIHQxLCB0MiksIGQoJ3RvdGFsJywgdDAsIHQyKV0uam9pbignLCAnKSk7XG5cbiAgICAgICAgcmVxdWVzdEFuaW1hdGlvbkZyYW1lKGRvVXBkYXRlKTtcbiAgICAgIH0gY2F0Y2ggKGUpIHtcbiAgICAgICAgY29uc29sZS5lcnJvcihlKTtcbiAgICAgICAgdGhpcy5pc1JlbmRlcmluZyA9IGZhbHNlO1xuICAgICAgICB0aGlzLnBvc3Rwcm9jZXNzQ2hhbmdlcygpO1xuICAgICAgfVxuICAgIH07XG4gICAgcmVxdWVzdEFuaW1hdGlvbkZyYW1lKGRvVXBkYXRlKTtcbiAgfVxuXG4gIHByaXZhdGUgcG9zdHByb2Nlc3NDaGFuZ2VzKCkge1xuICAgIHRoaXMuci5wb3N0UmVuZGVyKCk7XG5cbiAgICAvLyB0b2RvOiBhYnN0cmFjdCB0aGlzXG4gICAgaWYgKHRoaXMucHJldmlld01vZGUgPT09IFByZXZpZXdNb2RlLlNsaWRlKSB7XG4gICAgICBkb2N1bWVudC5xdWVyeVNlbGVjdG9yQWxsKCcudHlwc3QtcGFnZS1udW1iZXItaW5kaWNhdG9yJykuZm9yRWFjaCh4ID0+IHtcbiAgICAgICAgeC50ZXh0Q29udGVudCA9IGAke3RoaXMua01vZHVsZS5yZXRyaWV2ZVBhZ2VzSW5mbygpLmxlbmd0aH1gO1xuICAgICAgfSk7XG4gICAgfVxuICB9XG5cbiAgYWRkQ2hhbmdlbWVudChjaGFuZ2U6IFtzdHJpbmcsIHN0cmluZ10pIHtcbiAgICBpZiAoY2hhbmdlWzBdID09PSAnbmV3Jykge1xuICAgICAgdGhpcy5wYXRjaFF1ZXVlLnNwbGljZSgwLCB0aGlzLnBhdGNoUXVldWUubGVuZ3RoKTtcbiAgICB9XG5cbiAgICBjb25zdCBwdXNoQ2hhbmdlID0gKCkgPT4ge1xuICAgICAgdGhpcy52cFRpbWVvdXQgPSB1bmRlZmluZWQ7XG4gICAgICB0aGlzLnBhdGNoUXVldWUucHVzaChjaGFuZ2UpO1xuICAgICAgdGhpcy50cmlnZ2VyVXBkYXRlKCk7XG4gICAgfTtcblxuICAgIGlmICh0aGlzLnZwVGltZW91dCAhPT0gdW5kZWZpbmVkKSB7XG4gICAgICBjbGVhclRpbWVvdXQodGhpcy52cFRpbWVvdXQpO1xuICAgIH1cblxuICAgIGlmIChjaGFuZ2VbMF0gPT09ICd2aWV3cG9ydC1jaGFuZ2UnICYmIHRoaXMuaXNSZW5kZXJpbmcpIHtcbiAgICAgIC8vIGRlbGF5IHZpZXdwb3J0IGNoYW5nZSBhIGJpdFxuICAgICAgdGhpcy52cFRpbWVvdXQgPSBzZXRUaW1lb3V0KHB1c2hDaGFuZ2UsIHRoaXMuc2FtcGxlZFJlbmRlclRpbWUgfHwgMTAwKTtcbiAgICB9IGVsc2Uge1xuICAgICAgcHVzaENoYW5nZSgpO1xuICAgIH1cbiAgfVxuXG4gIGFkZFZpZXdwb3J0Q2hhbmdlKCkge1xuICAgIHRoaXMuYWRkQ2hhbmdlbWVudChbJ3ZpZXdwb3J0LWNoYW5nZScsICcnXSk7XG4gIH1cbn1cblxuZXhwb3J0IGludGVyZmFjZSBUeXBzdERvY3VtZW50PFQ+IHtcbiAgaW1wbDogVDtcbiAga01vZHVsZTogUmVuZGVyU2Vzc2lvbjtcbiAgZGlzcG9zZSgpOiB2b2lkO1xuICByZXNldCgpOiB2b2lkO1xuICBhZGRDaGFuZ2VtZW50KGNoYW5nZTogW3N0cmluZywgc3RyaW5nXSk6IHZvaWQ7XG4gIGFkZFZpZXdwb3J0Q2hhbmdlKCk6IHZvaWQ7XG4gIHNldFBhZ2VDb2xvcihjb2xvcjogc3RyaW5nKTogdm9pZDtcbiAgc2V0UGFydGlhbFJlbmRlcmluZyhwYXJ0aWFsUmVuZGVyaW5nOiBib29sZWFuKTogdm9pZDtcbiAgc2V0Q3Vyc29yKHBhZ2U6IG51bWJlciwgeDogbnVtYmVyLCB5OiBudW1iZXIpOiB2b2lkO1xuICBzZXRQYXJ0aWFsUGFnZU51bWJlcihwYWdlOiBudW1iZXIpOiBib29sZWFuO1xuICBnZXRQYXJ0aWFsUGFnZU51bWJlcigpOiBudW1iZXI7XG4gIHNldE91dGluZURhdGEob3V0bGluZTogYW55KTogdm9pZDtcbn1cblxuZXhwb3J0IGZ1bmN0aW9uIHByb3ZpZGVEb2M8VCBleHRlbmRzIFR5cHN0RG9jdW1lbnRDb250ZXh0PihcbiAgQmFzZTogR0NvbnN0cnVjdG9yPFQ+LFxuKTogbmV3IChvcHRpb25zOiBPcHRpb25zICYgVFsnb3B0cyddKSA9PiBUeXBzdERvY3VtZW50PFQ+IHtcbiAgcmV0dXJuIGNsYXNzIFR5cHN0RG9jdW1lbnQge1xuICAgIHB1YmxpYyBpbXBsOiBUO1xuICAgIHB1YmxpYyBrTW9kdWxlOiBSZW5kZXJTZXNzaW9uO1xuXG4gICAgY29uc3RydWN0b3Iob3B0aW9uczogT3B0aW9ucykge1xuICAgICAgaWYgKG9wdGlvbnMuaXNDb250ZW50UHJldmlldykge1xuICAgICAgICBvcHRpb25zLnJlbmRlck1vZGUgPSAnY2FudmFzJztcbiAgICAgIH1cblxuICAgICAgdGhpcy5rTW9kdWxlID0gb3B0aW9ucy5rTW9kdWxlO1xuICAgICAgdGhpcy5pbXBsID0gbmV3IEJhc2Uob3B0aW9ucyk7XG4gICAgICBpZiAoIXRoaXMuaW1wbC5yKSB7XG4gICAgICAgIHRocm93IG5ldyBFcnJvcihgbW9kZSBpcyBub3Qgc3VwcG9ydGVkLCAke29wdGlvbnM/LnJlbmRlck1vZGV9YCk7XG4gICAgICB9XG5cbiAgICAgIGlmIChvcHRpb25zLmlzQ29udGVudFByZXZpZXcpIHtcbiAgICAgICAgLy8gY29udGVudCBwcmV2aWV3IGhhcyB2ZXJ5IGJhZCBwZXJmb3JtYW5jZSB3aXRob3V0IHBhcnRpYWwgcmVuZGVyaW5nXG4gICAgICAgIHRoaXMuaW1wbC5wYXJ0aWFsUmVuZGVyaW5nID0gdHJ1ZTtcbiAgICAgICAgdGhpcy5pbXBsLnBpeGVsUGVyUHQgPSAxO1xuICAgICAgICB0aGlzLmltcGwuaXNNaXhpbk91dGxpbmUgPSB0cnVlO1xuICAgICAgfVxuICAgIH1cblxuICAgIGRpc3Bvc2UoKSB7XG4gICAgICB0aGlzLmltcGwuZGlzcG9zZSgpO1xuICAgIH1cblxuICAgIHJlc2V0KCkge1xuICAgICAgdGhpcy5pbXBsLnJlc2V0KCk7XG4gICAgfVxuXG4gICAgYWRkQ2hhbmdlbWVudChjaGFuZ2U6IFtzdHJpbmcsIHN0cmluZ10pIHtcbiAgICAgIHRoaXMuaW1wbC5hZGRDaGFuZ2VtZW50KGNoYW5nZSk7XG4gICAgfVxuXG4gICAgYWRkVmlld3BvcnRDaGFuZ2UoKSB7XG4gICAgICB0aGlzLmltcGwuYWRkVmlld3BvcnRDaGFuZ2UoKTtcbiAgICB9XG5cbiAgICBzZXRQYWdlQ29sb3IoY29sb3I6IHN0cmluZykge1xuICAgICAgdGhpcy5pbXBsLnBhZ2VDb2xvciA9IGNvbG9yO1xuICAgICAgdGhpcy5hZGRWaWV3cG9ydENoYW5nZSgpO1xuICAgIH1cblxuICAgIHNldFBhcnRpYWxSZW5kZXJpbmcocGFydGlhbFJlbmRlcmluZzogYm9vbGVhbikge1xuICAgICAgdGhpcy5pbXBsLnBhcnRpYWxSZW5kZXJpbmcgPSBwYXJ0aWFsUmVuZGVyaW5nO1xuICAgIH1cblxuICAgIHNldEN1cnNvcihwYWdlOiBudW1iZXIsIHg6IG51bWJlciwgeTogbnVtYmVyKSB7XG4gICAgICB0aGlzLmltcGwuY3Vyc29yUG9zaXRpb24gPSBbcGFnZSwgeCwgeV07XG4gICAgfVxuXG4gICAgc2V0UGFydGlhbFBhZ2VOdW1iZXIocGFnZTogbnVtYmVyKTogYm9vbGVhbiB7XG4gICAgICBpZiAocGFnZSA8PSAwIHx8IHBhZ2UgPiB0aGlzLmtNb2R1bGUucmV0cmlldmVQYWdlc0luZm8oKS5sZW5ndGgpIHtcbiAgICAgICAgcmV0dXJuIGZhbHNlO1xuICAgICAgfVxuICAgICAgdGhpcy5pbXBsLnBhcnRpYWxSZW5kZXJQYWdlID0gcGFnZSAtIDE7XG4gICAgICB0aGlzLmFkZFZpZXdwb3J0Q2hhbmdlKCk7XG4gICAgICByZXR1cm4gdHJ1ZTtcbiAgICB9XG5cbiAgICBnZXRQYXJ0aWFsUGFnZU51bWJlcigpOiBudW1iZXIge1xuICAgICAgcmV0dXJuIHRoaXMuaW1wbC5wYXJ0aWFsUmVuZGVyUGFnZSArIDE7XG4gICAgfVxuXG4gICAgc2V0T3V0aW5lRGF0YShvdXRsaW5lOiBhbnkpIHtcbiAgICAgIHRoaXMuaW1wbC5vdXRsaW5lID0gb3V0bGluZTtcbiAgICAgIHRoaXMuYWRkVmlld3BvcnRDaGFuZ2UoKTtcbiAgICB9XG4gIH07XG59XG5cbmV4cG9ydCBmdW5jdGlvbiBjb21wb3NlRG9jPFRCYXNlIGV4dGVuZHMgR0NvbnN0cnVjdG9yLCBGMT4oXG4gIEJhc2U6IFRCYXNlLFxuICBmMTogKGJhc2U6IFRCYXNlKSA9PiBGMSxcbik6IFRCYXNlICYgRjE7XG5leHBvcnQgZnVuY3Rpb24gY29tcG9zZURvYzxUQmFzZSBleHRlbmRzIEdDb25zdHJ1Y3RvciwgRjEsIEYyPihcbiAgQmFzZTogVEJhc2UsXG4gIGYxOiAoYmFzZTogVEJhc2UpID0+IEYxLFxuICBmMjogKGJhc2U6IEYxKSA9PiBGMixcbik6IFRCYXNlICYgRjEgJiBGMjtcbmV4cG9ydCBmdW5jdGlvbiBjb21wb3NlRG9jPFRCYXNlIGV4dGVuZHMgR0NvbnN0cnVjdG9yLCBGMSwgRjIsIEYzPihcbiAgQmFzZTogVEJhc2UsXG4gIGYxOiAoYmFzZTogVEJhc2UpID0+IEYxLFxuICBmMjogKGJhc2U6IEYxKSA9PiBGMixcbiAgZjM6IChiYXNlOiBGMikgPT4gRjMsXG4pOiBUQmFzZSAmIEYxICYgRjIgJiBGMztcbmV4cG9ydCBmdW5jdGlvbiBjb21wb3NlRG9jPFRCYXNlIGV4dGVuZHMgR0NvbnN0cnVjdG9yLCBGMSwgRjIsIEYzLCBGND4oXG4gIEJhc2U6IFRCYXNlLFxuICBmMTogKGJhc2U6IFRCYXNlKSA9PiBGMSxcbiAgZjI6IChiYXNlOiBGMSkgPT4gRjIsXG4gIGYzOiAoYmFzZTogRjIpID0+IEYzLFxuICBmNDogKGJhc2U6IEYzKSA9PiBGNCxcbik6IFRCYXNlICYgRjEgJiBGMiAmIEYzICYgRjQ7XG5leHBvcnQgZnVuY3Rpb24gY29tcG9zZURvYzxUQmFzZSBleHRlbmRzIEdDb25zdHJ1Y3RvciwgRjEsIEYyLCBGMywgRjQsIEY1PihcbiAgQmFzZTogVEJhc2UsXG4gIGYxOiAoYmFzZTogVEJhc2UpID0+IEYxLFxuICBmMjogKGJhc2U6IEYxKSA9PiBGMixcbiAgZjM6IChiYXNlOiBGMikgPT4gRjMsXG4gIGY0OiAoYmFzZTogRjMpID0+IEY0LFxuICBmNTogKGJhc2U6IEY0KSA9PiBGNSxcbik6IFRCYXNlICYgRjEgJiBGMiAmIEYzICYgRjQgJiBGNTtcbmV4cG9ydCBmdW5jdGlvbiBjb21wb3NlRG9jPFRCYXNlIGV4dGVuZHMgR0NvbnN0cnVjdG9yLCBGMSwgRjIsIEYzLCBGNCwgRjUsIEY2PihcbiAgQmFzZTogVEJhc2UsXG4gIGYxOiAoYmFzZTogVEJhc2UpID0+IEYxLFxuICBmMjogKGJhc2U6IEYxKSA9PiBGMixcbiAgZjM6IChiYXNlOiBGMikgPT4gRjMsXG4gIGY0OiAoYmFzZTogRjMpID0+IEY0LFxuICBmNTogKGJhc2U6IEY0KSA9PiBGNSxcbiAgZjY6IChiYXNlOiBGNSkgPT4gRjYsXG4pOiBUQmFzZSAmIEYxICYgRjIgJiBGMyAmIEY0ICYgRjUgJiBGNjtcbmV4cG9ydCBmdW5jdGlvbiBjb21wb3NlRG9jPFRCYXNlIGV4dGVuZHMgR0NvbnN0cnVjdG9yLCBGMSwgRjIsIEYzLCBGNCwgRjUsIEY2LCBGNz4oXG4gIEJhc2U6IFRCYXNlLFxuICBmMTogKGJhc2U6IFRCYXNlKSA9PiBGMSxcbiAgZjI6IChiYXNlOiBGMSkgPT4gRjIsXG4gIGYzOiAoYmFzZTogRjIpID0+IEYzLFxuICBmNDogKGJhc2U6IEYzKSA9PiBGNCxcbiAgZjU6IChiYXNlOiBGNCkgPT4gRjUsXG4gIGY2OiAoYmFzZTogRjUpID0+IEY2LFxuICBmNzogKGJhc2U6IEY2KSA9PiBGNyxcbik6IFRCYXNlICYgRjEgJiBGMiAmIEYzICYgRjQgJiBGNSAmIEY2ICYgRjc7XG5leHBvcnQgZnVuY3Rpb24gY29tcG9zZURvYzxUQmFzZSBleHRlbmRzIEdDb25zdHJ1Y3RvciwgRjEsIEYyLCBGMywgRjQsIEY1LCBGNiwgRjcsIEY4PihcbiAgQmFzZTogVEJhc2UsXG4gIGYxOiAoYmFzZTogVEJhc2UpID0+IEYxLFxuICBmMjogKGJhc2U6IEYxKSA9PiBGMixcbiAgZjM6IChiYXNlOiBGMikgPT4gRjMsXG4gIGY0OiAoYmFzZTogRjMpID0+IEY0LFxuICBmNTogKGJhc2U6IEY0KSA9PiBGNSxcbiAgZjY6IChiYXNlOiBGNSkgPT4gRjYsXG4gIGY3OiAoYmFzZTogRjYpID0+IEY3LFxuICBmODogKGJhc2U6IEY3KSA9PiBGOCxcbik6IFRCYXNlICYgRjEgJiBGMiAmIEYzICYgRjQgJiBGNSAmIEY2ICYgRjcgJiBGODtcbmV4cG9ydCBmdW5jdGlvbiBjb21wb3NlRG9jPFRCYXNlIGV4dGVuZHMgR0NvbnN0cnVjdG9yLCBGMSwgRjIsIEYzLCBGNCwgRjUsIEY2LCBGNywgRjgsIEY5PihcbiAgQmFzZTogVEJhc2UsXG4gIGYxOiAoYmFzZTogVEJhc2UpID0+IEYxLFxuICBmMjogKGJhc2U6IEYxKSA9PiBGMixcbiAgZjM6IChiYXNlOiBGMikgPT4gRjMsXG4gIGY0OiAoYmFzZTogRjMpID0+IEY0LFxuICBmNTogKGJhc2U6IEY0KSA9PiBGNSxcbiAgZjY6IChiYXNlOiBGNSkgPT4gRjYsXG4gIGY3OiAoYmFzZTogRjYpID0+IEY3LFxuICBmODogKGJhc2U6IEY3KSA9PiBGOCxcbiAgZjk6IChiYXNlOiBGOCkgPT4gRjksXG4pOiBUQmFzZSAmIEYxICYgRjIgJiBGMyAmIEY0ICYgRjUgJiBGNiAmIEY3ICYgRjggJiBGOTtcbmV4cG9ydCBmdW5jdGlvbiBjb21wb3NlRG9jPFRCYXNlIGV4dGVuZHMgR0NvbnN0cnVjdG9yPihCYXNlOiBUQmFzZSwgLi4ubWl4aW5zOiBhbnlbXSk6IFRCYXNlIHtcbiAgcmV0dXJuIG1peGlucy5yZWR1Y2UoKGFjYywgbWl4aW4pID0+IG1peGluKGFjYyksIEJhc2UpO1xufVxuIl19