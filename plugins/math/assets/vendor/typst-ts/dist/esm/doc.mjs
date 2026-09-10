import { patchRoot } from './render/svg/patch.mjs';
export class TypstDocument {
    props;
    session;
    sessionReady;
    disposeSession;
    constructor(props) {
        this.props = props;
        this.init();
    }
    static layoutPagesFullMode(doc) {
        return Promise.resolve(doc.session.retrievePagesInfo().map(() => {
            return {};
        }));
    }
    // copy from typst-preview
    static layoutSvgPagesPartialMode(doc, before) {
        const docRect = doc.shadowRoot.getBoundingClientRect(); // this.cachedBoundingRect;
        // https://measurethat.net/Benchmarks/Show/5392/0/clientwidth-vs-offsetwidth-vs-windowgetcomputedstyle
        // todo: this is only a PoC
        const cachedOffsetWidth = 'offsetWidth' in doc.shadowRoot
            ? doc.shadowRoot.offsetWidth
            : Number.parseInt(window.getComputedStyle(doc.shadowRoot).width.replace('px', ''));
        const currentScaleRatio = 1; // this.currentScaleRatio;
        // scale derived from svg width and container with.
        const computedRevScale = cachedOffsetWidth ? doc.session.docWidth / cachedOffsetWidth : 1;
        // respect current scale ratio
        const revScale = computedRevScale / currentScaleRatio;
        const left = (window.screenLeft - docRect.left) * revScale;
        const top = (window.screenTop - docRect.top) * revScale;
        const width = window.innerWidth * revScale;
        const height = window.innerHeight * revScale;
        void before;
        const patchStr = doc.session.renderSvgDiff({
            window: {
                lo: {
                    x: left,
                    y: top,
                },
                hi: {
                    x: left + width,
                    y: top + height,
                },
            },
        });
        // todo: ideally, we should patch per page separately
        // todo: then, we can call the api doc.renderPieces(pages + windows).
        if (doc.shadowRoot.firstElementChild) {
            const elem = document.createElement('div');
            elem.innerHTML = patchStr;
            const svgElement = elem.firstElementChild;
            patchRoot(doc.shadowRoot.firstElementChild, svgElement);
        }
        else {
            doc.shadowRoot.innerHTML = patchStr;
        }
        return Promise.resolve([]);
    }
    // copy from typst-preview
    // todo: generalize patchRoot here
    static layoutSvgPages(options) {
        return (doc, before) => {
            // todo
            return TypstDocument.layoutSvgPagesPartialMode(doc, before);
        };
    }
    addChangements(change) {
        throw new Error('Method not implemented.');
    }
    renderPieces(pieces) {
        throw new Error('Method not implemented.');
    }
    onSessionReady() {
        return this.sessionReady;
    }
    dispose() {
        if (this.disposeSession !== undefined) {
            this.disposeSession();
        }
    }
    init() {
        if (this.sessionReady !== undefined) {
            throw new Error('Already initialized');
        }
        if (this.props.session !== undefined) {
            this.session = this.props.session;
            return (this.sessionReady = Promise.resolve(this.session));
        }
        return (this.sessionReady = new Promise(resolve => {
            this.props.plugin.runWithSession(session => {
                return new Promise(dispose => {
                    this.session = session;
                    this.disposeSession = dispose;
                    resolve(session);
                });
            });
        }));
    }
}
//   private collectDocProperties(): DocProperties {
//     return {
//       [kRects]: undefined,
//     };
//   }
//   async mutate(
//     cb: (ctx: PageMutationContext) => Promise<void>,
//     // options?: {
//     //   prefetchedDocProperties?: DocProperties;
//     // },
//   ): Promise<void> {
//     await this.sessionReady;
//     // const docProperties = options?.prefetchedDocProperties ?? this.collectDocProperties();
//     const ctx = new PageMutationContextImpl(this.props, docProperties);
//     await cb(ctx);
//     return ctx[kDispose]();
//   }
// import { ManipulateDataOptions } from '../../options.render';
// export interface PageMutationInst<K> {
//   /**
//    * The kind of mutation.
//    * @property {string} kind - The kind of mutation.
//    */
//   kind: K;
//   /**
//    * After/At which element the mutation happens.
//    */
//   sentinel?: Element;
// }
// export type PageMutation = PageMutationInst<'create' | 'delete' | 'change'>;
// const kRects = Symbol('rects');
// class PageMutationContextImpl implements PageMutationContext {
//   mutations: PageMutation[] = [];
//   constructor(
//     private props: TypstDocumentProps,
//     private docProperties: DocProperties,
//   ) {}
//   manipulateData(opts: ManipulateDataOptions) {
//     throw new Error('Method not implemented.');
//   }
//   //   mutateSeq(mutation: PageMutation[]): Promise<Element[]> {
//   //     return Promise.all(mutation.map(this.mutateOne));
//   //   }
//   //   mutateOne(mutation: PageMutation): Promise<Element> {
//   //     throw new Error('Method not implemented. ' + JSON.stringify(mutation));
//   //   }
//   [kDispose](): void {}
// }
// export interface DocProperties {
//   [kRects]: unknown;
// }
//# sourceMappingURL=data:application/json;base64,eyJ2ZXJzaW9uIjozLCJmaWxlIjoiZG9jLm1qcyIsInNvdXJjZVJvb3QiOiIiLCJzb3VyY2VzIjpbIi4uLy4uL3NyYy9kb2MubXRzIl0sIm5hbWVzIjpbXSwibWFwcGluZ3MiOiJBQUVBLE9BQU8sRUFBRSxTQUFTLEVBQUUsTUFBTSx3QkFBd0IsQ0FBQztBQStFbkQsTUFBTSxPQUFPLGFBQWE7SUFLSjtJQUpwQixPQUFPLENBQWdCO0lBQ2YsWUFBWSxDQUF5QjtJQUNyQyxjQUFjLENBQTBCO0lBRWhELFlBQW9CLEtBQXdDO1FBQXhDLFVBQUssR0FBTCxLQUFLLENBQW1DO1FBQzFELElBQUksQ0FBQyxJQUFJLEVBQUUsQ0FBQztJQUNkLENBQUM7SUFFRCxNQUFNLENBQUMsbUJBQW1CLENBQUMsR0FBa0I7UUFDM0MsT0FBTyxPQUFPLENBQUMsT0FBTyxDQUNwQixHQUFHLENBQUMsT0FBTyxDQUFDLGlCQUFpQixFQUFFLENBQUMsR0FBRyxDQUFDLEdBQUcsRUFBRTtZQUN2QyxPQUFPLEVBQUUsQ0FBQztRQUNaLENBQUMsQ0FBQyxDQUNILENBQUM7SUFDSixDQUFDO0lBRUQsMEJBQTBCO0lBQzFCLE1BQU0sQ0FBQyx5QkFBeUIsQ0FBQyxHQUFrQixFQUFFLE1BQWdCO1FBQ25FLE1BQU0sT0FBTyxHQUFHLEdBQUcsQ0FBQyxVQUFVLENBQUMscUJBQXFCLEVBQUUsQ0FBQyxDQUFDLDJCQUEyQjtRQUNuRixzR0FBc0c7UUFDdEcsMkJBQTJCO1FBQzNCLE1BQU0saUJBQWlCLEdBQ3JCLGFBQWEsSUFBSyxHQUFHLENBQUMsVUFBMEI7WUFDOUMsQ0FBQyxDQUFFLEdBQUcsQ0FBQyxVQUEwQixDQUFDLFdBQVc7WUFDN0MsQ0FBQyxDQUFDLE1BQU0sQ0FBQyxRQUFRLENBQUMsTUFBTSxDQUFDLGdCQUFnQixDQUFDLEdBQUcsQ0FBQyxVQUFVLENBQUMsQ0FBQyxLQUFLLENBQUMsT0FBTyxDQUFDLElBQUksRUFBRSxFQUFFLENBQUMsQ0FBQyxDQUFDO1FBQ3ZGLE1BQU0saUJBQWlCLEdBQUcsQ0FBQyxDQUFDLENBQUMs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Q0FBeUM7QUFDekMsNENBQTRDO0FBQzVDLFNBQVM7QUFFVCxrREFBa0Q7QUFDbEQsa0RBQWtEO0FBQ2xELE1BQU07QUFFTixtRUFBbUU7QUFDbkUsNkRBQTZEO0FBQzdELFdBQVc7QUFFWCwrREFBK0Q7QUFDL0QsbUZBQW1GO0FBQ25GLFdBQVc7QUFFWCwwQkFBMEI7QUFDMUIsSUFBSTtBQUVKLG1DQUFtQztBQUNuQyx1QkFBdUI7QUFDdkIsSUFBSSIsInNvdXJjZXNDb250ZW50IjpbImltcG9ydCB7IFBhZ2VJbmZvLCBSZWN0LCBUcmFuc2Zvcm1NYXRyaXggfSBmcm9tICcuL2ludGVybmFsLnR5cGVzLm1qcyc7XG5pbXBvcnQgeyBSZW5kZXJPcHRpb25zIH0gZnJvbSAnLi9vcHRpb25zLnJlbmRlci5tanMnO1xuaW1wb3J0IHsgcGF0Y2hSb290IH0gZnJvbSAnLi9yZW5kZXIvc3ZnL3BhdGNoLm1qcyc7XG5pbXBvcnQgeyBSZW5kZXJTZXNzaW9uLCBUeXBzdFJlbmRlcmVyIH0gZnJvbSAnLi9yZW5kZXJlci5tanMnO1xuXG5leHBvcnQgaW50ZXJmYWNlIExheW91dENvbnRleHQge31cblxuZXhwb3J0IGludGVyZmFjZSBQYWdlVmlldyB7XG4gIHBhZ2VzOiBQYWdlSW5mb1tdO1xufVxuXG5leHBvcnQgaW50ZXJmYWNlIFJlbmRlclBpZWNlIHtcbiAgcGFnZU9mZnNldD86IG51bWJlcjtcbiAgaW52aXNpYmxlPzogYm9vbGVhbjtcbiAgYXQ/OiBFbGVtZW50O1xuICB3aW5kb3c/OiBSZWN0O1xuICB0cz86IFRyYW5zZm9ybU1hdHJpeDtcbn1cblxuZXhwb3J0IGludGVyZmFjZSBUeXBzdERvY3VtZW50UHJvcHMge1xuICBwbHVnaW46IFR5cHN0UmVuZGVyZXI7XG4gIHNoYWRvd1Jvb3Q6IEVsZW1lbnQ7XG4gIC8vIGRlZmF1bHQ6IHN2Z1xuICByZW5kZXJNb2RlPzogJ3N2ZycgfCAnc3ZnLWdyb3VwJyB8ICdjYW52YXMnO1xuXG4gIHNlc3Npb24/OiBSZW5kZXJTZXNzaW9uO1xuXG4gIGxheW91dFBhZ2VzPyhkb2M6IFR5cHN0RG9jdW1lbnQsIGJlZm9yZTogUGFnZVZpZXcpOiBQcm9taXNlPFJlbmRlclBpZWNlW10+O1xufVxuXG4vKipcbiAqIFRoZSBvcHRpb25zIGZvciBtYW5pcHVsYXRpbmcgdGhlIFR5cHN0IGRvY3VtZW50IGluIHRoZSBzZXNzaW9uLlxuICovXG5pbnRlcmZhY2UgRG9jdW1lbnREYXRhQ2hhbmdlbWVudCB7XG4gIC8qKlxuICAgKiBUaGUgYWN0aW9uIHRvIG1hbmlwdWxhdGUgdGhlIGRhdGEuXG4gICAqIEBkZXNjcmlwdGlvbiBgcmVzZXQtZG9jYDogcmVzZXQgdGhlIGRhdGEgdG8gdGhlIGluaXRpYWwgc3RhdGUuXG4gICAqIEBkZXNjcmlwdGlvbiBgbWVyZ2UtZG9jYDogbWVyZ2UgdGhlIGRhdGEgdG8gdGhlIGN1cnJlbnQgc3RhdGUuXG4gICAqL1xuICBhY3Rpb246ICdyZXNldC1kb2MnIHwgJ21lcmdlLWRvYyc7XG4gIC8qKlxuICAgKiBPcGFxdWUgZGF0YSB0byBtYW5pcHVsYXRlIHRoZSBUeXBzdCBkb2N1bWVudCBmcm9tIHNlcnZlci5cbiAgICovXG4gIGRhdGE6IFVpbnQ4QXJyYXk7XG59XG5cbi8qKlxuICogVGhlIG9wdGlvbnMgZm9yIG1hbmlwdWxhdGluZyB0aGUgVHlwc3QgZG9jdW1lbnQgaW4gdGhlIHNlc3Npb24uXG4gKi9cbmludGVyZmFjZSBEb2N1bWVudFZpZXdwb3J0Q2hhbmdlbWVudCB7XG4gIC8qKlxuICAgKiBDaGFuZ2UgdGhlIHZpZXdwb3J0IG9mIHRoZSBUeXBzdCBkb2N1bWVudC5cbiAgICovXG4gIGFjdGlvbjogJ3ZpZXdwb3J0LWNoYW5nZSc7XG59XG5cbi8qKlxuICogVGhlIG9wdGlvbnMgZm9yIG1hbmlwdWxhdGluZyB0aGUgVHlwc3QgZG9jdW1lbnQgaW4gdGhlIHNlc3Npb24uXG4gKi9cbmV4cG9ydCB0eXBlIERvY3VtZW50Q2hhbmdlbWVudCA9IERvY3VtZW50RGF0YUNoYW5nZW1lbnQgfCBEb2N1bWVudFZpZXdwb3J0Q2hhbmdlbWVudDtcblxuZXhwb3J0IGludGVyZmFjZSBMYXlvdXRTdmdQYWdlT3B0aW9ucyB7XG4gIC8vLyB3cmFwIGluIHN2Zy1ncm91cCBtb2RlXG4gIHdyYXBHPyhnOiBbUmVjdCwgU1ZHR0VsZW1lbnRdKTogW1JlY3QsIFNWR0dFbGVtZW50XTtcbn1cblxuZXhwb3J0IGludGVyZmFjZSBUeXBzdERvY3VtZW50IHtcbiAgc2hhZG93Um9vdDogRWxlbWVudDtcbiAgc2Vzc2lvbjogUmVuZGVyU2Vzc2lvbjtcbiAgb25TZXNzaW9uUmVhZHkoKTogUHJvbWlzZTxSZW5kZXJTZXNzaW9uPjtcblxuICBjaGFuZ2VMYXlvdXQobGF5b3V0U2VsZWN0b3I6IFJlY29yZDxzdHJpbmcsIGFueT4pOiB2b2lkO1xuXG4gIGFkZENoYW5nZW1lbnRzKGNoYW5nZTogRG9jdW1lbnRDaGFuZ2VtZW50W10pOiB2b2lkO1xuICBhZGRWaWV3cG9ydENoYW5nZSgpOiB2b2lkO1xuXG4gIHJlbmRlclBpZWNlcyhwaWVjZXM6IFJlbmRlclBpZWNlW10pOiBQcm9taXNlPHZvaWQ+O1xuXG4gIGRpc3Bvc2UoKTogdm9pZDtcbn1cblxuZXhwb3J0IGNsYXNzIFR5cHN0RG9jdW1lbnQgaW1wbGVtZW50cyBUeXBzdERvY3VtZW50IHtcbiAgc2Vzc2lvbjogUmVuZGVyU2Vzc2lvbjtcbiAgcHJpdmF0ZSBzZXNzaW9uUmVhZHk6IFByb21pc2U8UmVuZGVyU2Vzc2lvbj47XG4gIHByaXZhdGUgZGlzcG9zZVNlc3Npb246ICguLi5hcmdzOiBhbnlbXSkgPT4gYW55O1xuXG4gIGNvbnN0cnVjdG9yKHByaXZhdGUgcHJvcHM6IFJlbmRlck9wdGlvbnM8VHlwc3REb2N1bWVudFByb3BzPikge1xuICAgIHRoaXMuaW5pdCgpO1xuICB9XG5cbiAgc3RhdGljIGxheW91dFBhZ2VzRnVsbE1vZGUoZG9jOiBUeXBzdERvY3VtZW50KTogUHJvbWlzZTxSZW5kZXJQaWVjZVtdPiB7XG4gICAgcmV0dXJuIFByb21pc2UucmVzb2x2ZShcbiAgICAgIGRvYy5zZXNzaW9uLnJldHJpZXZlUGFnZXNJbmZvKCkubWFwKCgpID0+IHtcbiAgICAgICAgcmV0dXJuIHt9O1xuICAgICAgfSksXG4gICAgKTtcbiAgfVxuXG4gIC8vIGNvcHkgZnJvbSB0eXBzdC1wcmV2aWV3XG4gIHN0YXRpYyBsYXlvdXRTdmdQYWdlc1BhcnRpYWxNb2RlKGRvYzogVHlwc3REb2N1bWVudCwgYmVmb3JlOiBQYWdlVmlldyk6IFByb21pc2U8UmVuZGVyUGllY2VbXT4ge1xuICAgIGNvbnN0IGRvY1JlY3QgPSBkb2Muc2hhZG93Um9vdC5nZXRCb3VuZGluZ0NsaWVudFJlY3QoKTsgLy8gdGhpcy5jYWNoZWRCb3VuZGluZ1JlY3Q7XG4gICAgLy8gaHR0cHM6Ly9tZWFzdXJldGhhdC5uZXQvQmVuY2htYXJrcy9TaG93LzUzOTIvMC9jbGllbnR3aWR0aC12cy1vZmZzZXR3aWR0aC12cy13aW5kb3dnZXRjb21wdXRlZHN0eWxlXG4gICAgLy8gdG9kbzogdGhpcyBpcyBvbmx5IGEgUG9DXG4gICAgY29uc3QgY2FjaGVkT2Zmc2V0V2lkdGg6IG51bWJlciA9XG4gICAgICAnb2Zmc2V0V2lkdGgnIGluIChkb2Muc2hhZG93Um9vdCBhcyBIVE1MRWxlbWVudClcbiAgICAgICAgPyAoZG9jLnNoYWRvd1Jvb3QgYXMgSFRNTEVsZW1lbnQpLm9mZnNldFdpZHRoXG4gICAgICAgIDogTnVtYmVyLnBhcnNlSW50KHdpbmRvdy5nZXRDb21wdXRlZFN0eWxlKGRvYy5zaGFkb3dSb290KS53aWR0aC5yZXBsYWNlKCdweCcsICcnKSk7XG4gICAgY29uc3QgY3VycmVudFNjYWxlUmF0aW8gPSAxOyAvLyB0aGlzLmN1cnJlbnRTY2FsZVJhdGlvO1xuICAgIC8vIHNjYWxlIGRlcml2ZWQgZnJvbSBzdmcgd2lkdGggYW5kIGNvbnRhaW5lciB3aXRoLlxuICAgIGNvbnN0IGNvbXB1dGVkUmV2U2NhbGUgPSBjYWNoZWRPZmZzZXRXaWR0aCA/IGRvYy5zZXNzaW9uLmRvY1dpZHRoIC8gY2FjaGVkT2Zmc2V0V2lkdGggOiAxO1xuICAgIC8vIHJlc3BlY3QgY3VycmVudCBzY2FsZSByYXRpb1xuICAgIGNvbnN0IHJldlNjYWxlID0gY29tcHV0ZWRSZXZTY2FsZSAvIGN1cnJlbnRTY2FsZVJhdGlvO1xuICAgIGNvbnN0IGxlZnQgPSAod2luZG93LnNjcmVlbkxlZnQgLSBkb2NSZWN0LmxlZnQpICogcmV2U2NhbGU7XG4gICAgY29uc3QgdG9wID0gKHdpbmRvdy5zY3JlZW5Ub3AgLSBkb2NSZWN0LnRvcCkgKiByZXZTY2FsZTtcbiAgICBjb25zdCB3aWR0aCA9IHdpbmRvdy5pbm5lcldpZHRoICogcmV2U2NhbGU7XG4gICAgY29uc3QgaGVpZ2h0ID0gd2luZG93LmlubmVySGVpZ2h0ICogcmV2U2NhbGU7XG5cbiAgICB2b2lkIGJlZm9yZTtcbiAgICBjb25zdCBwYXRjaFN0ciA9IGRvYy5zZXNzaW9uLnJlbmRlclN2Z0RpZmYoe1xuICAgICAgd2luZG93OiB7XG4gICAgICAgIGxvOiB7XG4gICAgICAgICAgeDogbGVmdCxcbiAgICAgICAgICB5OiB0b3AsXG4gICAgICAgIH0sXG4gICAgICAgIGhpOiB7XG4gICAgICAgICAgeDogbGVmdCArIHdpZHRoLFxuICAgICAgICAgIHk6IHRvcCArIGhlaWdodCxcbiAgICAgICAgfSxcbiAgICAgIH0sXG4gICAgfSk7XG4gICAgLy8gdG9kbzogaWRlYWxseSwgd2Ugc2hvdWxkIHBhdGNoIHBlciBwYWdlIHNlcGFyYXRlbHlcbiAgICAvLyB0b2RvOiB0aGVuLCB3ZSBjYW4gY2FsbCB0aGUgYXBpIGRvYy5yZW5kZXJQaWVjZXMocGFnZXMgKyB3aW5kb3dzKS5cblxuICAgIGlmIChkb2Muc2hhZG93Um9vdC5maXJzdEVsZW1lbnRDaGlsZCkge1xuICAgICAgY29uc3QgZWxlbSA9IGRvY3VtZW50LmNyZWF0ZUVsZW1lbnQoJ2RpdicpO1xuICAgICAgZWxlbS5pbm5lckhUTUwgPSBwYXRjaFN0cjtcbiAgICAgIGNvbnN0IHN2Z0VsZW1lbnQgPSBlbGVtLmZpcnN0RWxlbWVudENoaWxkIGFzIFNWR0VsZW1lbnQ7XG4gICAgICBwYXRjaFJvb3QoZG9jLnNoYWRvd1Jvb3QuZmlyc3RFbGVtZW50Q2hpbGQgYXMgU1ZHRWxlbWVudCwgc3ZnRWxlbWVudCk7XG4gICAgfSBlbHNlIHtcbiAgICAgIGRvYy5zaGFkb3dSb290LmlubmVySFRNTCA9IHBhdGNoU3RyO1xuICAgIH1cblxuICAgIHJldHVybiBQcm9taXNlLnJlc29sdmUoW10pO1xuICB9XG5cbiAgLy8gY29weSBmcm9tIHR5cHN0LXByZXZpZXdcbiAgLy8gdG9kbzogZ2VuZXJhbGl6ZSBwYXRjaFJvb3QgaGVyZVxuICBzdGF0aWMgbGF5b3V0U3ZnUGFnZXMoXG4gICAgb3B0aW9uczogTGF5b3V0U3ZnUGFnZU9wdGlvbnMsXG4gICk6IChkb2M6IFR5cHN0RG9jdW1lbnQsIGJlZm9yZTogUGFnZVZpZXcpID0+IFByb21pc2U8UmVuZGVyUGllY2VbXT4ge1xuICAgIHJldHVybiAoZG9jOiBUeXBzdERvY3VtZW50LCBiZWZvcmU6IFBhZ2VWaWV3KSA9PiB7XG4gICAgICAvLyB0b2RvXG4gICAgICByZXR1cm4gVHlwc3REb2N1bWVudC5sYXlvdXRTdmdQYWdlc1BhcnRpYWxNb2RlKGRvYywgYmVmb3JlKTtcbiAgICB9O1xuICB9XG5cbiAgYWRkQ2hhbmdlbWVudHMoY2hhbmdlOiBEb2N1bWVudENoYW5nZW1lbnRbXSk6IHZvaWQge1xuICAgIHRocm93IG5ldyBFcnJvcignTWV0aG9kIG5vdCBpbXBsZW1lbnRlZC4nKTtcbiAgfVxuXG4gIHJlbmRlclBpZWNlcyhwaWVjZXM6IFJlbmRlclBpZWNlW10pOiBQcm9taXNlPHZvaWQ+IHtcbiAgICB0aHJvdyBuZXcgRXJyb3IoJ01ldGhvZCBub3QgaW1wbGVtZW50ZWQuJyk7XG4gIH1cblxuICBvblNlc3Npb25SZWFkeSgpOiBQcm9taXNlPFJlbmRlclNlc3Npb24+IHtcbiAgICByZXR1cm4gdGhpcy5zZXNzaW9uUmVhZHk7XG4gIH1cblxuICBkaXNwb3NlKCkge1xuICAgIGlmICh0aGlzLmRpc3Bvc2VTZXNzaW9uICE9PSB1bmRlZmluZWQpIHtcbiAgICAgIHRoaXMuZGlzcG9zZVNlc3Npb24oKTtcbiAgICB9XG4gIH1cblxuICBwcml2YXRlIGluaXQoKTogUHJvbWlzZTxSZW5kZXJTZXNzaW9uPiB7XG4gICAgaWYgKHRoaXMuc2Vzc2lvblJlYWR5ICE9PSB1bmRlZmluZWQpIHtcbiAgICAgIHRocm93IG5ldyBFcnJvcignQWxyZWFkeSBpbml0aWFsaXplZCcpO1xuICAgIH1cblxuICAgIGlmICh0aGlzLnByb3BzLnNlc3Npb24gIT09IHVuZGVmaW5lZCkge1xuICAgICAgdGhpcy5zZXNzaW9uID0gdGhpcy5wcm9wcy5zZXNzaW9uO1xuICAgICAgcmV0dXJuICh0aGlzLnNlc3Npb25SZWFkeSA9IFByb21pc2UucmVzb2x2ZSh0aGlzLnNlc3Npb24pKTtcbiAgICB9XG5cbiAgICByZXR1cm4gKHRoaXMuc2Vzc2lvblJlYWR5ID0gbmV3IFByb21pc2UocmVzb2x2ZSA9PiB7XG4gICAgICB0aGlzLnByb3BzLnBsdWdpbi5ydW5XaXRoU2Vzc2lvbihzZXNzaW9uID0+IHtcbiAgICAgICAgcmV0dXJuIG5ldyBQcm9taXNlKGRpc3Bvc2UgPT4ge1xuICAgICAgICAgIHRoaXMuc2Vzc2lvbiA9IHNlc3Npb247XG4gICAgICAgICAgdGhpcy5kaXNwb3NlU2Vzc2lvbiA9IGRpc3Bvc2U7XG4gICAgICAgICAgcmVzb2x2ZShzZXNzaW9uKTtcbiAgICAgICAgfSk7XG4gICAgICB9KTtcbiAgICB9KSk7XG4gIH1cbn1cblxuLy8gICBwcml2YXRlIGNvbGxlY3REb2NQcm9wZXJ0aWVzKCk6IERvY1Byb3BlcnRpZXMge1xuLy8gICAgIHJldHVybiB7XG4vLyAgICAgICBba1JlY3RzXTogdW5kZWZpbmVkLFxuLy8gICAgIH07XG4vLyAgIH1cblxuLy8gICBhc3luYyBtdXRhdGUoXG4vLyAgICAgY2I6IChjdHg6IFBhZ2VNdXRhdGlvbkNvbnRleHQpID0+IFByb21pc2U8dm9pZD4sXG4vLyAgICAgLy8gb3B0aW9ucz86IHtcbi8vICAgICAvLyAgIHByZWZldGNoZWREb2NQcm9wZXJ0aWVzPzogRG9jUHJvcGVydGllcztcbi8vICAgICAvLyB9LFxuLy8gICApOiBQcm9taXNlPHZvaWQ+IHtcbi8vICAgICBhd2FpdCB0aGlzLnNlc3Npb25SZWFkeTtcblxuLy8gICAgIC8vIGNvbnN0IGRvY1Byb3BlcnRpZXMgPSBvcHRpb25zPy5wcmVmZXRjaGVkRG9jUHJvcGVydGllcyA/PyB0aGlzLmNvbGxlY3REb2NQcm9wZXJ0aWVzKCk7XG4vLyAgICAgY29uc3QgY3R4ID0gbmV3IFBhZ2VNdXRhdGlvbkNvbnRleHRJbXBsKHRoaXMucHJvcHMsIGRvY1Byb3BlcnRpZXMpO1xuLy8gICAgIGF3YWl0IGNiKGN0eCk7XG4vLyAgICAgcmV0dXJuIGN0eFtrRGlzcG9zZV0oKTtcbi8vICAgfVxuXG4vLyBpbXBvcnQgeyBNYW5pcHVsYXRlRGF0YU9wdGlvbnMgfSBmcm9tICcuLi8uLi9vcHRpb25zLnJlbmRlcic7XG4vLyBleHBvcnQgaW50ZXJmYWNlIFBhZ2VNdXRhdGlvbkluc3Q8Sz4ge1xuLy8gICAvKipcbi8vICAgICogVGhlIGtpbmQgb2YgbXV0YXRpb24uXG4vLyAgICAqIEBwcm9wZXJ0eSB7c3RyaW5nfSBraW5kIC0gVGhlIGtpbmQgb2YgbXV0YXRpb24uXG4vLyAgICAqL1xuLy8gICBraW5kOiBLO1xuXG4vLyAgIC8qKlxuLy8gICAgKiBBZnRlci9BdCB3aGljaCBlbGVtZW50IHRoZSBtdXRhdGlvbiBoYXBwZW5zLlxuLy8gICAgKi9cbi8vICAgc2VudGluZWw/OiBFbGVtZW50O1xuLy8gfVxuXG4vLyBleHBvcnQgdHlwZSBQYWdlTXV0YXRpb24gPSBQYWdlTXV0YXRpb25JbnN0PCdjcmVhdGUnIHwgJ2RlbGV0ZScgfCAnY2hhbmdlJz47XG4vLyBjb25zdCBrUmVjdHMgPSBTeW1ib2woJ3JlY3RzJyk7XG5cbi8vIGNsYXNzIFBhZ2VNdXRhdGlvbkNvbnRleHRJbXBsIGltcGxlbWVudHMgUGFnZU11dGF0aW9uQ29udGV4dCB7XG4vLyAgIG11dGF0aW9uczogUGFnZU11dGF0aW9uW10gPSBbXTtcbi8vICAgY29uc3RydWN0b3IoXG4vLyAgICAgcHJpdmF0ZSBwcm9wczogVHlwc3REb2N1bWVudFByb3BzLFxuLy8gICAgIHByaXZhdGUgZG9jUHJvcGVydGllczogRG9jUHJvcGVydGllcyxcbi8vICAgKSB7fVxuXG4vLyAgIG1hbmlwdWxhdGVEYXRhKG9wdHM6IE1hbmlwdWxhdGVEYXRhT3B0aW9ucykge1xuLy8gICAgIHRocm93IG5ldyBFcnJvcignTWV0aG9kIG5vdCBpbXBsZW1lbnRlZC4nKTtcbi8vICAgfVxuXG4vLyAgIC8vICAgbXV0YXRlU2VxKG11dGF0aW9uOiBQYWdlTXV0YXRpb25bXSk6IFByb21pc2U8RWxlbWVudFtdPiB7XG4vLyAgIC8vICAgICByZXR1cm4gUHJvbWlzZS5hbGwobXV0YXRpb24ubWFwKHRoaXMubXV0YXRlT25lKSk7XG4vLyAgIC8vICAgfVxuXG4vLyAgIC8vICAgbXV0YXRlT25lKG11dGF0aW9uOiBQYWdlTXV0YXRpb24pOiBQcm9taXNlPEVsZW1lbnQ+IHtcbi8vICAgLy8gICAgIHRocm93IG5ldyBFcnJvcignTWV0aG9kIG5vdCBpbXBsZW1lbnRlZC4gJyArIEpTT04uc3RyaW5naWZ5KG11dGF0aW9uKSk7XG4vLyAgIC8vICAgfVxuXG4vLyAgIFtrRGlzcG9zZV0oKTogdm9pZCB7fVxuLy8gfVxuXG4vLyBleHBvcnQgaW50ZXJmYWNlIERvY1Byb3BlcnRpZXMge1xuLy8gICBba1JlY3RzXTogdW5rbm93bjtcbi8vIH1cbiJdfQ==