/**
 * this mark is used to identify the beforeBuild stage
 * @description will not be used in runtime code
 */
const BeforeBuildSymbol = Symbol('beforeBuild');
/** @internal */
const _textFonts = [
    'DejaVuSansMono-Bold.ttf',
    'DejaVuSansMono-BoldOblique.ttf',
    'DejaVuSansMono-Oblique.ttf',
    'DejaVuSansMono.ttf',
    'LibertinusSerif-Bold.otf',
    'LibertinusSerif-BoldItalic.otf',
    'LibertinusSerif-Italic.otf',
    'LibertinusSerif-Regular.otf',
    'LibertinusSerif-Semibold.otf',
    'LibertinusSerif-SemiboldItalic.otf',
    'NewCM10-Bold.otf',
    'NewCM10-BoldItalic.otf',
    'NewCM10-Italic.otf',
    'NewCM10-Regular.otf',
    'NewCMMath-Bold.otf',
    'NewCMMath-Book.otf',
    'NewCMMath-Regular.otf',
];
/** @internal */
const _cjkFonts = [
    'InriaSerif-Bold.ttf',
    'InriaSerif-BoldItalic.ttf',
    'InriaSerif-Italic.ttf',
    'InriaSerif-Regular.ttf',
    'Roboto-Regular.ttf',
    'NotoSerifCJKsc-Regular.otf',
];
/** @internal */
const _emojiFonts = ['TwitterColorEmoji.ttf', 'NotoColorEmoji-Regular-COLR.subset.ttf'];
/**
 * disable default font assets
 */
export function disableDefaultFontAssets() {
    return loadFonts([], { assets: false });
}
/**
 * preload font assets
 */
export function preloadFontAssets(options) {
    return loadFonts([], options);
}
export function _resolveAssets(options) {
    const fonts = [];
    if (options &&
        options?.assets !== false &&
        options?.assets?.length &&
        options?.assets?.length > 0) {
        let defaultPrefix = {
            text: 'https://cdn.jsdelivr.net/gh/typst/typst-assets@v0.13.1/files/fonts/',
            _: 'https://cdn.jsdelivr.net/gh/typst/typst-dev-assets@v0.13.1/files/fonts/',
        };
        let assetUrlPrefix = options.assetUrlPrefix ?? defaultPrefix;
        if (typeof assetUrlPrefix === 'string') {
            assetUrlPrefix = { _: assetUrlPrefix };
        }
        else {
            assetUrlPrefix = { ...defaultPrefix, ...assetUrlPrefix };
        }
        for (const key of Object.keys(assetUrlPrefix)) {
            const u = assetUrlPrefix[key];
            if (u[u.length - 1] !== '/') {
                assetUrlPrefix[key] = u + '/';
            }
        }
        const prefix = (asset, f) => f.map(font => (assetUrlPrefix[asset] || assetUrlPrefix['_']) + font);
        for (const asset of options.assets) {
            switch (asset) {
                case 'text':
                    fonts.push(...prefix(asset, _textFonts));
                    break;
                case 'cjk':
                    fonts.push(...prefix(asset, _cjkFonts));
                    break;
                case 'emoji':
                    fonts.push(...prefix(asset, _emojiFonts));
                    break;
            }
        }
    }
    return fonts;
}
/**
 * @deprecated use {@link loadFonts} instead
 */
export function preloadRemoteFonts(userFonts, options) {
    return loadFonts(userFonts, options);
}
/**
 * load fonts
 *
 * @param fonts - url path to font files
 * @returns {BeforeBuildFn}
 * @example
 * ```ts
 * // preLoad fonts from remote url (because finto info is not provided)
 * import { init, loadFonts } from 'typst';
 * init({
 *   beforeBuild: [
 *     loadFonts([
 *      'https://fonts.gstatic.com/s/roboto/v27/KFOmCnqEu92Fr1Mu4mxKKTU1Kg.woff2', // remote url
 *      'dist/fonts/Roboto-Regular.ttf', // relative to the root of the website
 *     ]),
 *   ],
 * });
 * ```
 * @example
 * ```ts
 * // lazily Load fonts from remote url. The font information is obtained by `getFontInfo`
 * import { init, loadFonts } from 'typst';
 * init({
 *   beforeBuild: [
 *     loadFonts([
 *      {
 *        info: [...]
 *        url: 'https://fonts.gstatic.com/s/roboto/v27/KFOmCnqEu92Fr1Mu4mxKKTU1Kg.woff2';
 *      }
 *     ]),
 *   ],
 * });
 * ```
 */
export function loadFonts(userFonts, options) {
    const assetFonts = _resolveAssets(options);
    const loader = async (_, { ref, builder }) => {
        if (options?.fetcher) {
            ref.setFetcher(options.fetcher);
        }
        await ref.loadFonts(builder, [...userFonts, ...assetFonts]);
    };
    loader._preloadRemoteFontOptions = options;
    loader._kind = 'fontLoader';
    return loader;
}
/**
 * preload system fonts
 * @param byFamily - filter system fonts to preload by family name
 * @returns {BeforeBuildFn}
 * @example
 * ```typescript
 * import { init, preloadSystemFonts } from 'typst';
 * init({
 *   beforeBuild: [
 *     preloadSystemFonts({
 *       byFamily: ['Roboto'], // preload fonts by family name
 *     }),
 *   ],
 * });
 * ```
 */
export function preloadSystemFonts({ byFamily }) {
    return async (_, { builder }) => {
        const t = performance.now();
        if ('queryLocalFonts' in window) {
            const fonts = await window.queryLocalFonts();
            byFamily = byFamily ?? [];
            for (const font of fonts) {
                if (!byFamily.includes(font.family)) {
                    continue;
                }
                const data = await (await font.blob()).arrayBuffer();
                await builder.add_raw_font(new Uint8Array(data));
            }
        }
        const t2 = performance.now();
        console.log('preload system font time used:', t2 - t);
    };
}
/**
 * (compile only) set pacoage registry
 *
 * @param accessModel: when compiling, the pacoage registry is used to access the
 * data of files
 * @returns {BeforeBuildFn}
 */
export function withPackageRegistry(packageRegistry) {
    return async (_, { builder }) => {
        return new Promise(resolve => {
            builder.set_package_registry(packageRegistry, function (spec) {
                return packageRegistry.resolve(spec, this);
            });
            resolve();
        });
    };
}
/**
 * (compile only) set access model
 *
 * @param accessModel: when compiling, the access model is used to access the
 * data of files
 * @returns {BeforeBuildFn}
 */
export function withAccessModel(accessModel) {
    return async (_, ctx) => {
        if (ctx.alreadySetAccessModel) {
            throw new Error(`already set some assess model before: ${ctx.alreadySetAccessModel.constructor?.name}(${ctx.alreadySetAccessModel})`);
        }
        ctx.alreadySetAccessModel = accessModel;
        return new Promise(resolve => {
            ctx.builder.set_access_model(accessModel, (path) => {
                const lastModified = accessModel.getMTime(path);
                if (lastModified) {
                    return lastModified.getTime();
                }
                return 0;
            }, (path) => {
                return accessModel.isFile(path) || false;
            }, (path) => {
                return accessModel.getRealPath(path) || path;
            }, (path) => {
                return accessModel.readAll(path);
            });
            resolve();
        });
    };
}
// todo: search browser
// searcher.search_browser().await?;
//# sourceMappingURL=data:application/json;base64,eyJ2ZXJzaW9uIjozLCJmaWxlIjoib3B0aW9ucy5pbml0Lm1qcyIsInNvdXJjZVJvb3QiOiIiLCJzb3VyY2VzIjpbIi4uLy4uL3NyYy9vcHRpb25zLmluaXQubXRzIl0sIm5hbWVzIjpbXSwibWFwcGluZ3MiOiJBQWFBOzs7R0FHRztBQUNILE1BQU0saUJBQWlCLEdBQWtCLE1BQU0sQ0FBQyxhQUFhLENBQUMsQ0FBQztBQTREL0QsZ0JBQWdCO0FBQ2hCLE1BQU0sVUFBVSxHQUFhO0lBQzNCLHlCQUF5QjtJQUN6QixnQ0FBZ0M7SUFDaEMsNEJBQTRCO0lBQzVCLG9CQUFvQjtJQUNwQiwwQkFBMEI7SUFDMUIsZ0NBQWdDO0lBQ2hDLDRCQUE0QjtJQUM1Qiw2QkFBNkI7SUFDN0IsOEJBQThCO0lBQzlCLG9DQUFvQztJQUNwQyxrQkFBa0I7SUFDbEIsd0JBQXdCO0lBQ3hCLG9CQUFvQjtJQUNwQixxQkFBcUI7SUFDckIsb0JBQW9CO0lBQ3BCLG9CQUFvQjtJQUNwQix1QkFBdUI7Q0FDeEIsQ0FBQztBQUNGLGdCQUFnQjtBQUNoQixNQUFNLFNBQVMsR0FBYTtJQUMxQixxQkFBcUI7SUFDckIsMkJBQTJCO0lBQzNCLHVCQUF1QjtJQUN2Qix3QkFBd0I7SUFDeEIsb0JBQW9CO0lBQ3BCLDRCQUE0QjtDQUM3QixDQUFDO0FBQ0YsZ0JBQWdCO0FBQ2hCLE1BQU0sV0FBVyxHQUFhLENBQUMsdUJBQXVCLEVBQUUsd0NBQXdDLENBQUMsQ0FBQztBQW1DbEc7O0dBRUc7QUFDSCxNQUFNLFVBQVUsd0JBQXdCO0lBQ3RDLE9BQU8sU0FBUyxDQUFDLEVBQUUsRUFBRSxFQUFFLE1BQU0sRUFBRSxLQUFLLEVBQUUsQ0FBQyxDQUFDO0FBQzFDLENBQUM7QUFFRDs7R0FFRztBQUNILE1BQU0sVUFBVSxpQkFBaUIsQ0FBQyxPQUFpQztJQUNqRSxPQUFPLFNBQVMsQ0FBQyxFQUFFLEVBQUUsT0FBTyxDQUFDLENBQUM7QUFDaEMsQ0FBQztBQUVELE1BQU0sVUFBVSxjQUFjLENBQUMsT0FBZ0M7SUFDN0QsTUFBTSxLQUFLLEdBQUcs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TUFBTSxDQUFDLElBQUksQ0FBQyxJQUFJLEtBQUssQ0FBQztZQUMzQyxDQUFDLEVBQ0QsQ0FBQyxJQUFZLEVBQUUsRUFBRTtnQkFDZixPQUFPLFdBQVcsQ0FBQyxXQUFXLENBQUMsSUFBSSxDQUFDLElBQUksSUFBSSxDQUFDO1lBQy9DLENBQUMsRUFDRCxDQUFDLElBQVksRUFBRSxFQUFFO2dCQUNmLE9BQU8sV0FBVyxDQUFDLE9BQU8sQ0FBQyxJQUFJLENBQUMsQ0FBQztZQUNuQyxDQUFDLENBQ0YsQ0FBQztZQUNGLE9BQU8sRUFBRSxDQUFDO1FBQ1osQ0FBQyxDQUFDLENBQUM7SUFDTCxDQUFDLENBQUM7QUFDSixDQUFDO0FBbUJELHVCQUF1QjtBQUN2QixvQ0FBb0MiLCJzb3VyY2VzQ29udGVudCI6WyIvLyBAdHMtaWdub3JlXG5pbXBvcnQgdHlwZSAqIGFzIHR5cHN0UmVuZGVyZXIgZnJvbSAnQG15cmlhZGRyZWFtaW4vdHlwc3QtdHMtcmVuZGVyZXInO1xuaW1wb3J0IHR5cGUgKiBhcyB0eXBzdENvbXBpbGVyIGZyb20gJ0BteXJpYWRkcmVhbWluL3R5cHN0LXRzLXdlYi1jb21waWxlcic7XG5pbXBvcnQgdHlwZSB7IEZzQWNjZXNzTW9kZWwsIFBhY2thZ2VSZWdpc3RyeSwgUGFja2FnZVNwZWMgfSBmcm9tICcuL2ludGVybmFsLnR5cGVzLm1qcyc7XG5pbXBvcnQgdHlwZSB7IFdlYkFzc2VtYmx5TW9kdWxlUmVmIH0gZnJvbSAnLi93YXNtLm1qcyc7XG5cbi8qKlxuICogc3RhZ2VkIG9wdGlvbnMgZnVuY3Rpb25cbiAqIEB0ZW1wbGF0ZSBTIC0gc3RhZ2UgbWFya1xuICogQHRlbXBsYXRlIFQgLSBjb250ZXh0IHR5cGVcbiAqL1xuZXhwb3J0IHR5cGUgU3RhZ2VkT3B0Rm48UyBleHRlbmRzIHN5bWJvbCwgVCA9IGFueT4gPSAoczogUywgdDogVCkgPT4gUHJvbWlzZTx2b2lkPjtcblxuLyoqXG4gKiB0aGlzIG1hcmsgaXMgdXNlZCB0byBpZGVudGlmeSB0aGUgYmVmb3JlQnVpbGQgc3RhZ2VcbiAqIEBkZXNjcmlwdGlvbiB3aWxsIG5vdCBiZSB1c2VkIGluIHJ1bnRpbWUgY29kZVxuICovXG5jb25zdCBCZWZvcmVCdWlsZFN5bWJvbDogdW5pcXVlIHN5bWJvbCA9IFN5bWJvbCgnYmVmb3JlQnVpbGQnKTtcblxuLyoqXG4gKiB0aGlzIG1hcmsgaXMgdXNlZCB0byBpZGVudGlmeSB0aGUgYmVmb3JlQnVpbGQgc3RhZ2VcbiAqIEBkZXNjcmlwdGlvbiBjYW5ub3QgYmUgY3JlYXRlZCBieSBhbnkgcnVudGltZSBjb2RlXG4gKi9cbmV4cG9ydCB0eXBlIEJlZm9yZUJ1aWxkTWFyayA9IHR5cGVvZiBCZWZvcmVCdWlsZFN5bWJvbDtcblxuLyoqXG4gKiBiZWZvcmUgYnVpbGQgc3RhZ2VcbiAqIEBkZXNjcmlwdGlvbiBwb3NzaWJsZSBjcmVhdGVkIGJ5OlxuICogICAtIGxvYWRGb250c1xuICogICAtIHByZWxvYWRTeXN0ZW1Gb250c1xuICogICAtIHdpdGhBY2Nlc3NNb2RlbFxuICogICAtIHdpdGhQYWNrYWdlUmVnaXN0cnlcbiAqL1xuZXhwb3J0IHR5cGUgQmVmb3JlQnVpbGRGbiA9IFN0YWdlZE9wdEZuPEJlZm9yZUJ1aWxkTWFyaz47XG5cbi8qKlxuICpcbiAqIEBwcm9wZXJ0eSB7QmVmb3JlQnVpbGRGbltdfSBiZWZvcmVCdWlsZCAtIGNhbGxiYWNrcyBiZWZvcmUgYnVpbGQgc3RhZ2VcbiAqL1xuZXhwb3J0IGludGVyZmFjZSBJbml0T3B0aW9ucyB7XG4gIC8qKlxuICAgKiBjYWxsYmFja3MgYmVmb3JlIGJ1aWxkIHN0YWdlXG4gICAqXG4gICAqIGJlZm9yZSBidWlsZCBzdGFnZSwgdGhlIHJlZ2lzdGVyZWQgZnVuY3Rpb25zIHdpbGwgYmUgZXhlY3V0ZWQgaW4gb3JkZXJcbiAgICogcG9zc2libGUgb3B0aW9uczpcbiAgICogLSBsb2FkRm9udHNcbiAgICogLSBwcmVsb2FkU3lzdGVtRm9udHNcbiAgICogLSB3aXRoQWNjZXNzTW9kZWxcbiAgICovXG4gIGJlZm9yZUJ1aWxkOiBCZWZvcmVCdWlsZEZuW107XG5cbiAgLyoqXG4gICAqIGNhbGxiYWNrcyB0byBmZXRjaCB0aGUgd2FzbSBtb2R1bGUgd3JhcHBlclxuICAgKi9cbiAgZ2V0V3JhcHBlcj8oKTogUHJvbWlzZTxhbnk+O1xuXG4gIC8qKlxuICAgKiBjYWxsYmFja3MgdG8gZmV0Y2ggdGhlIHdhc20gbW9kdWxlXG4gICAqXG4gICAqIFRoZXJlIGFyZSBtYW55IHdheXMgdG8gcHJvdmlkZSBhIHdhc20gbW9kdWxlLCBzZWVcbiAgICoge0BsaW5rIFdlYkFzc2VtYmx5TW9kdWxlUmVmfSBmb3IgbW9yZSBkZXRhaWxzLiBJZiB5b3UgZG9uJ3QgcHJvdmlkZSBhIHdhc21cbiAgICogbW9kdWxlLCB0aGUgZGVmYXVsdCBtb2R1bGUgd2lsbCBiZSB1c2VkLlxuICAgKi9cbiAgZ2V0TW9kdWxlKCk6IFdlYkFzc2VtYmx5TW9kdWxlUmVmIHwgUHJvbWlzZTxXZWJBc3NlbWJseU1vZHVsZVJlZj47XG59XG5cbmV4cG9ydCB0eXBlIExhenlGb250ID0ge1xuICBpbmZvOiBhbnk7XG59ICYgKFxuICAgIHwge1xuICAgICAgYmxvYjogKGluZGV4OiBudW1iZXIpID0+IFVpbnQ4QXJyYXk7XG4gICAgfVxuICAgIHwge1xuICAgICAgdXJsOiBzdHJpbmc7XG4gICAgfVxuICApO1xuXG4vKiogQGludGVybmFsICovXG5jb25zdCBfdGV4dEZvbnRzOiBzdHJpbmdbXSA9IFtcbiAgJ0RlamFWdVNhbnNNb25vLUJvbGQudHRmJyxcbiAgJ0RlamFWdVNhbnNNb25vLUJvbGRPYmxpcXVlLnR0ZicsXG4gICdEZWphVnVTYW5zTW9uby1PYmxpcXVlLnR0ZicsXG4gICdEZWphVnVTYW5zTW9uby50dGYnLFxuICAnTGliZXJ0aW51c1NlcmlmLUJvbGQub3RmJyxcbiAgJ0xpYmVydGludXNTZXJpZi1Cb2xkSXRhbGljLm90ZicsXG4gICdMaWJlcnRpbnVzU2VyaWYtSXRhbGljLm90ZicsXG4gICdMaWJlcnRpbnVzU2VyaWYtUmVndWxhci5vdGYnLFxuICAnTGliZXJ0aW51c1NlcmlmLVNlbWlib2xkLm90ZicsXG4gICdMaWJlcnRpbnVzU2VyaWYtU2VtaWJvbGRJdGFsaWMub3RmJyxcbiAgJ05ld0NNMTAtQm9sZC5vdGYnLFxuICAnTmV3Q00xMC1Cb2xkSXRhbGljLm90ZicsXG4gICdOZXdDTTEwLUl0YWxpYy5vdGYnLFxuICAnTmV3Q00xMC1SZWd1bGFyLm90ZicsXG4gICdOZXdDTU1hdGgtQm9sZC5vdGYnLFxuICAnTmV3Q01NYXRoLUJvb2sub3RmJyxcbiAgJ05ld0NNTWF0aC1SZWd1bGFyLm90ZicsXG5dO1xuLyoqIEBpbnRlcm5hbCAqL1xuY29uc3QgX2Nqa0ZvbnRzOiBzdHJpbmdbXSA9IFtcbiAgJ0lucmlhU2VyaWYtQm9sZC50dGYnLFxuICAnSW5yaWFTZXJpZi1Cb2xkSXRhbGljLnR0ZicsXG4gICdJbnJpYVNlcmlmLUl0YWxpYy50dGYnLFxuICAnSW5yaWFTZXJpZi1SZWd1bGFyLnR0ZicsXG4gICdSb2JvdG8tUmVndWxhci50dGYnLFxuICAnTm90b1NlcmlmQ0pLc2MtUmVndWxhci5vdGYnLFxuXTtcbi8qKiBAaW50ZXJuYWwgKi9cbmNvbnN0IF9lbW9qaUZvbnRzOiBzdHJpbmdbXSA9IFsnVHdpdHRlckNvbG9yRW1vamkudHRmJywgJ05vdG9Db2xvckVtb2ppLVJlZ3VsYXItQ09MUi5zdWJzZXQudHRmJ107XG5cbnR5cGUgQXZhaWxhYmxlRm9udEFzc2V0ID0gJ3RleHQnIHwgJ2NqaycgfCAnZW1vamknO1xuXG5leHBvcnQgaW50ZXJmYWNlIExvYWRSZW1vdGVBc3NldHNPcHRpb25zIHtcbiAgLyoqXG4gICAqIHByZWxvYWQgZm9udCBhc3NldHMgb3IgZG9uJ3QgcHJlbG9hZCBhbnkgZm9udCBhc3NldHNcbiAgICogQGRlZmF1bHQgWyd0ZXh0J11cbiAgICovXG4gIGFzc2V0cz86IEF2YWlsYWJsZUZvbnRBc3NldFtdIHwgZmFsc2U7XG5cbiAgLyoqXG4gICAqIGN1c3RvbWl6ZSB1cmwgcHJlZml4IGZvciBkZWZhdWx0IGFzc2V0cyBmcm9tIHJlbW90ZVxuICAgKlxuICAgKiBUaGUgZGVmYXVsdCBhc3NldHMgYXJlIGhvc3RlZCBvbiBnaXRodWIsIHlvdSBjYW4gZG93bmxvYWQgdGhlbSBhbmQgaG9zdFxuICAgKiB0aGVtIG9uIHlvdXIgb3duIHNlcnZlciwgd2hpY2ggaXMgbW9yZSBwcmFjdGljYWwgZm9yIHByb2R1Y3Rpb24uXG4gICAqXG4gICAqIEhvc3RlZCBhdDogaHR0cHM6Ly9naXRodWIuY29tL015cmlhZC1EcmVhbWluL3R5cHN0L3RyZWUvYXNzZXRzLWZvbnRzXG4gICAqIExpc3Qgb2YgYXNzZXRzOlxuICAgKiBTZWUge0BsaW5rIF90ZXh0Rm9udHN9LCB7QGxpbmsgX2Nqa0ZvbnRzfSwgYW5kIHtAbGluayBfZW1vamlGb250c31cbiAgICpcbiAgICogQGRlZmF1bHQgJ2pzZGVsaXZyLXVybCBvZiB0eXBzdC1hc3NldHMgYW5kIHR5cHN0LWRldi1hc3NldHMnXG4gICAqL1xuICBhc3NldFVybFByZWZpeD86IHN0cmluZyB8IFJlY29yZDxzdHJpbmcsIHN0cmluZz47XG5cbiAgLyoqXG4gICAqIGN1c3RvbSBmZXRjaGVyXG4gICAqIE5vdGU6IHRoZSBkZWZhdWx0IGZldGNoZXIgZm9yIG5vZGUuanMgZG9lcyBub3QgY2FjaGUgYW55IGZvbnRzXG4gICAqIEBkZWZhdWx0IGZldGNoXG4gICAqL1xuICBmZXRjaGVyPzogdHlwZW9mIGZldGNoO1xufVxuXG5leHBvcnQgaW50ZXJmYWNlIExvYWRSZW1vdGVGb250c09wdGlvbnMgZXh0ZW5kcyBMb2FkUmVtb3RlQXNzZXRzT3B0aW9ucyB7IH1cblxuLyoqXG4gKiBkaXNhYmxlIGRlZmF1bHQgZm9udCBhc3NldHNcbiAqL1xuZXhwb3J0IGZ1bmN0aW9uIGRpc2FibGVEZWZhdWx0Rm9udEFzc2V0cygpOiBCZWZvcmVCdWlsZEZuIHtcbiAgcmV0dXJuIGxvYWRGb250cyhbXSwgeyBhc3NldHM6IGZhbHNlIH0pO1xufVxuXG4vKipcbiAqIHByZWxvYWQgZm9udCBhc3NldHNcbiAqL1xuZXhwb3J0IGZ1bmN0aW9uIHByZWxvYWRGb250QXNzZXRzKG9wdGlvbnM/OiBMb2FkUmVtb3RlQXNzZXRzT3B0aW9ucyk6IEJlZm9yZUJ1aWxkRm4ge1xuICByZXR1cm4gbG9hZEZvbnRzKFtdLCBvcHRpb25zKTtcbn1cblxuZXhwb3J0IGZ1bmN0aW9uIF9yZXNvbHZlQXNzZXRzKG9wdGlvbnM/OiBMb2FkUmVtb3RlRm9udHNPcHRpb25zKSB7XG4gIGNvbnN0IGZvbnRzID0gW107XG4gIGlmIChcbiAgICBvcHRpb25zICYmXG4gICAgb3B0aW9ucz8uYXNzZXRzICE9PSBmYWxzZSAmJlxuICAgIG9wdGlvbnM/LmFzc2V0cz8ubGVuZ3RoICYmXG4gICAgb3B0aW9ucz8uYXNzZXRzPy5sZW5ndGggPiAwXG4gICkge1xuICAgIGxldCBkZWZhdWx0UHJlZml4OiBSZWNvcmQ8c3RyaW5nLCBzdHJpbmc+ID0ge1xuICAgICAgdGV4dDogJ2h0dHBzOi8vY2RuLmpzZGVsaXZyLm5ldC9naC90eXBzdC90eXBzdC1hc3NldHNAdjAuMTMuMS9maWxlcy9mb250cy8nLFxuICAgICAgXzogJ2h0dHBzOi8vY2RuLmpzZGVsaXZyLm5ldC9naC90eXBzdC90eXBzdC1kZXYtYXNzZXRzQHYwLjEzLjEvZmlsZXMvZm9udHMvJyxcbiAgICB9O1xuICAgIGxldCBhc3NldFVybFByZWZpeCA9IG9wdGlvbnMuYXNzZXRVcmxQcmVmaXggPz8gZGVmYXVsdFByZWZpeDtcbiAgICBpZiAodHlwZW9mIGFzc2V0VXJsUHJlZml4ID09PSAnc3RyaW5nJykge1xuICAgICAgYXNzZXRVcmxQcmVmaXggPSB7IF86IGFzc2V0VXJsUHJlZml4IH07XG4gICAgfSBlbHNlIHtcbiAgICAgIGFzc2V0VXJsUHJlZml4ID0geyAuLi5kZWZhdWx0UHJlZml4LCAuLi5hc3NldFVybFByZWZpeCB9O1xuICAgIH1cbiAgICBmb3IgKGNvbnN0IGtleSBvZiBPYmplY3Qua2V5cyhhc3NldFVybFByZWZpeCkpIHtcbiAgICAgIGNvbnN0IHUgPSBhc3NldFVybFByZWZpeFtrZXldO1xuICAgICAgaWYgKHVbdS5sZW5ndGggLSAxXSAhPT0gJy8nKSB7XG4gICAgICAgIGFzc2V0VXJsUHJlZml4W2tleV0gPSB1ICsgJy8nO1xuICAgICAgfVxuICAgIH1cblxuICAgIGNvbnN0IHByZWZpeCA9IChhc3NldDogc3RyaW5nLCBmOiBzdHJpbmdbXSkgPT5cbiAgICAgIGYubWFwKGZvbnQgPT4gKGFzc2V0VXJsUHJlZml4W2Fzc2V0XSB8fCBhc3NldFVybFByZWZpeFsnXyddKSArIGZvbnQpO1xuICAgIGZvciAoY29uc3QgYXNzZXQgb2Ygb3B0aW9ucy5hc3NldHMpIHtcbiAgICAgIHN3aXRjaCAoYXNzZXQpIHtcbiAgICAgICAgY2FzZSAndGV4dCc6XG4gICAgICAgICAgZm9udHMucHVzaCguLi5wcmVmaXgoYXNzZXQsIF90ZXh0Rm9udHMpKTtcbiAgICAgICAgICBicmVhaztcbiAgICAgICAgY2FzZSAnY2prJzpcbiAgICAgICAgICBmb250cy5wdXNoKC4uLnByZWZpeChhc3NldCwgX2Nqa0ZvbnRzKSk7XG4gICAgICAgICAgYnJlYWs7XG4gICAgICAgIGNhc2UgJ2Vtb2ppJzpcbiAgICAgICAgICBmb250cy5wdXNoKC4uLnByZWZpeChhc3NldCwgX2Vtb2ppRm9udHMpKTtcbiAgICAgICAgICBicmVhaztcbiAgICAgIH1cbiAgICB9XG4gIH1cblxuICByZXR1cm4gZm9udHM7XG59XG5cbi8qKlxuICogQGRlcHJlY2F0ZWQgdXNlIHtAbGluayBsb2FkRm9udHN9IGluc3RlYWRcbiAqL1xuZXhwb3J0IGZ1bmN0aW9uIHByZWxvYWRSZW1vdGVGb250cyhcbiAgdXNlckZvbnRzOiAoc3RyaW5nIHwgVWludDhBcnJheSlbXSxcbiAgb3B0aW9ucz86IExvYWRSZW1vdGVGb250c09wdGlvbnMsXG4pOiBCZWZvcmVCdWlsZEZuIHtcbiAgcmV0dXJuIGxvYWRGb250cyh1c2VyRm9udHMsIG9wdGlvbnMpO1xufVxuXG4vKipcbiAqIGxvYWQgZm9udHNcbiAqXG4gKiBAcGFyYW0gZm9udHMgLSB1cmwgcGF0aCB0byBmb250IGZpbGVzXG4gKiBAcmV0dXJucyB7QmVmb3JlQnVpbGRGbn1cbiAqIEBleGFtcGxlXG4gKiBgYGB0c1xuICogLy8gcHJlTG9hZCBmb250cyBmcm9tIHJlbW90ZSB1cmwgKGJlY2F1c2UgZmludG8gaW5mbyBpcyBub3QgcHJvdmlkZWQpXG4gKiBpbXBvcnQgeyBpbml0LCBsb2FkRm9udHMgfSBmcm9tICd0eXBzdCc7XG4gKiBpbml0KHtcbiAqICAgYmVmb3JlQnVpbGQ6IFtcbiAqICAgICBsb2FkRm9udHMoW1xuICogICAgICAnaHR0cHM6Ly9mb250cy5nc3RhdGljLmNvbS9zL3JvYm90by92MjcvS0ZPbUNucUV1OTJGcjFNdTRteEtLVFUxS2cud29mZjInLCAvLyByZW1vdGUgdXJsXG4gKiAgICAgICdkaXN0L2ZvbnRzL1JvYm90by1SZWd1bGFyLnR0ZicsIC8vIHJlbGF0aXZlIHRvIHRoZSByb290IG9mIHRoZSB3ZWJzaXRlXG4gKiAgICAgXSksXG4gKiAgIF0sXG4gKiB9KTtcbiAqIGBgYFxuICogQGV4YW1wbGVcbiAqIGBgYHRzXG4gKiAvLyBsYXppbHkgTG9hZCBmb250cyBmcm9tIHJlbW90ZSB1cmwuIFRoZSBmb250IGluZm9ybWF0aW9uIGlzIG9idGFpbmVkIGJ5IGBnZXRGb250SW5mb2BcbiAqIGltcG9ydCB7IGluaXQsIGxvYWRGb250cyB9IGZyb20gJ3R5cHN0JztcbiAqIGluaXQoe1xuICogICBiZWZvcmVCdWlsZDogW1xuICogICAgIGxvYWRGb250cyhbXG4gKiAgICAgIHtcbiAqICAgICAgICBpbmZvOiBbLi4uXVxuICogICAgICAgIHVybDogJ2h0dHBzOi8vZm9udHMuZ3N0YXRpYy5jb20vcy9yb2JvdG8vdjI3L0tGT21DbnFFdTkyRnIxTXU0bXhLS1RVMUtnLndvZmYyJztcbiAqICAgICAgfVxuICogICAgIF0pLFxuICogICBdLFxuICogfSk7XG4gKiBgYGBcbiAqL1xuZXhwb3J0IGZ1bmN0aW9uIGxvYWRGb250cyhcbiAgdXNlckZvbnRzOiAoc3RyaW5nIHwgVWludDhBcnJheSB8IExhenlGb250KVtdLFxuICBvcHRpb25zPzogTG9hZFJlbW90ZUZvbnRzT3B0aW9ucyxcbik6IEJlZm9yZUJ1aWxkRm4ge1xuICBjb25zdCBhc3NldEZvbnRzID0gX3Jlc29sdmVBc3NldHMob3B0aW9ucyk7XG4gIGNvbnN0IGxvYWRlciA9IGFzeW5jIChfOiBCZWZvcmVCdWlsZE1hcmssIHsgcmVmLCBidWlsZGVyIH06IEluaXRDb250ZXh0KSA9PiB7XG4gICAgaWYgKG9wdGlvbnM/LmZldGNoZXIpIHtcbiAgICAgIHJlZi5zZXRGZXRjaGVyKG9wdGlvbnMuZmV0Y2hlcik7XG4gICAgfVxuICAgIGF3YWl0IHJlZi5sb2FkRm9udHMoYnVpbGRlciwgWy4uLnVzZXJGb250cywgLi4uYXNzZXRGb250c10pO1xuICB9O1xuICBsb2FkZXIuX3ByZWxvYWRSZW1vdGVGb250T3B0aW9ucyA9IG9wdGlvbnM7XG4gIGxvYWRlci5fa2luZCA9ICdmb250TG9hZGVyJztcbiAgcmV0dXJuIGxvYWRlcjtcbn1cblxuLyoqXG4gKiBwcmVsb2FkIHN5c3RlbSBmb250c1xuICogQHBhcmFtIGJ5RmFtaWx5IC0gZmlsdGVyIHN5c3RlbSBmb250cyB0byBwcmVsb2FkIGJ5IGZhbWlseSBuYW1lXG4gKiBAcmV0dXJucyB7QmVmb3JlQnVpbGRGbn1cbiAqIEBleGFtcGxlXG4gKiBgYGB0eXBlc2NyaXB0XG4gKiBpbXBvcnQgeyBpbml0LCBwcmVsb2FkU3lzdGVtRm9udHMgfSBmcm9tICd0eXBzdCc7XG4gKiBpbml0KHtcbiAqICAgYmVmb3JlQnVpbGQ6IFtcbiAqICAgICBwcmVsb2FkU3lzdGVtRm9udHMoe1xuICogICAgICAgYnlGYW1pbHk6IFsnUm9ib3RvJ10sIC8vIHByZWxvYWQgZm9udHMgYnkgZmFtaWx5IG5hbWVcbiAqICAgICB9KSxcbiAqICAgXSxcbiAqIH0pO1xuICogYGBgXG4gKi9cbmV4cG9ydCBmdW5jdGlvbiBwcmVsb2FkU3lzdGVtRm9udHMoeyBieUZhbWlseSB9OiB7IGJ5RmFtaWx5Pzogc3RyaW5nW10gfSk6IEJlZm9yZUJ1aWxkRm4ge1xuICByZXR1cm4gYXN5bmMgKF8sIHsgYnVpbGRlciB9OiBJbml0Q29udGV4dCkgPT4ge1xuICAgIGNvbnN0IHQgPSBwZXJmb3JtYW5jZS5ub3coKTtcblxuICAgIGlmICgncXVlcnlMb2NhbEZvbnRzJyBpbiB3aW5kb3cpIHtcbiAgICAgIGNvbnN0IGZvbnRzOiB7XG4gICAgICAgIGZhbWlseTogc3RyaW5nO1xuICAgICAgICBibG9iKCk6IFByb21pc2U8QmxvYj47XG4gICAgICB9W10gPSBhd2FpdCAod2luZG93IGFzIGFueSkucXVlcnlMb2NhbEZvbnRzKCk7XG5cbiAgICAgIGJ5RmFtaWx5ID0gYnlGYW1pbHkgPz8gW107XG5cbiAgICAgIGZvciAoY29uc3QgZm9udCBvZiBmb250cykge1xuICAgICAgICBpZiAoIWJ5RmFtaWx5LmluY2x1ZGVzKGZvbnQuZmFtaWx5KSkge1xuICAgICAgICAgIGNvbnRpbnVlO1xuICAgICAgICB9XG5cbiAgICAgICAgY29uc3QgZGF0YTogQXJyYXlCdWZmZXIgPSBhd2FpdCAoYXdhaXQgZm9udC5ibG9iKCkpLmFycmF5QnVmZmVyKCk7XG4gICAgICAgIGF3YWl0IGJ1aWxkZXIuYWRkX3Jhd19mb250KG5ldyBVaW50OEFycmF5KGRhdGEpKTtcbiAgICAgIH1cbiAgICB9XG5cbiAgICBjb25zdCB0MiA9IHBlcmZvcm1hbmNlLm5vdygpO1xuICAgIGNvbnNvbGUubG9nKCdwcmVsb2FkIHN5c3RlbSBmb250IHRpbWUgdXNlZDonLCB0MiAtIHQpO1xuICB9O1xufVxuXG4vKipcbiAqIChjb21waWxlIG9ubHkpIHNldCBwYWNvYWdlIHJlZ2lzdHJ5XG4gKlxuICogQHBhcmFtIGFjY2Vzc01vZGVsOiB3aGVuIGNvbXBpbGluZywgdGhlIHBhY29hZ2UgcmVnaXN0cnkgaXMgdXNlZCB0byBhY2Nlc3MgdGhlXG4gKiBkYXRhIG9mIGZpbGVzXG4gKiBAcmV0dXJucyB7QmVmb3JlQnVpbGRGbn1cbiAqL1xuZXhwb3J0IGZ1bmN0aW9uIHdpdGhQYWNrYWdlUmVnaXN0cnkocGFja2FnZVJlZ2lzdHJ5OiBQYWNrYWdlUmVnaXN0cnkpOiBCZWZvcmVCdWlsZEZuIHtcbiAgcmV0dXJuIGFzeW5jIChfLCB7IGJ1aWxkZXIgfTogSW5pdENvbnRleHQpID0+IHtcbiAgICByZXR1cm4gbmV3IFByb21pc2UocmVzb2x2ZSA9PiB7XG4gICAgICBidWlsZGVyLnNldF9wYWNrYWdlX3JlZ2lzdHJ5KHBhY2thZ2VSZWdpc3RyeSwgZnVuY3Rpb24gKHNwZWM6IFBhY2thZ2VTcGVjKSB7XG4gICAgICAgIHJldHVybiBwYWNrYWdlUmVnaXN0cnkucmVzb2x2ZShzcGVjLCB0aGlzKTtcbiAgICAgIH0pO1xuICAgICAgcmVzb2x2ZSgpO1xuICAgIH0pO1xuICB9O1xufVxuXG4vKipcbiAqIChjb21waWxlIG9ubHkpIHNldCBhY2Nlc3MgbW9kZWxcbiAqXG4gKiBAcGFyYW0gYWNjZXNzTW9kZWw6IHdoZW4gY29tcGlsaW5nLCB0aGUgYWNjZXNzIG1vZGVsIGlzIHVzZWQgdG8gYWNjZXNzIHRoZVxuICogZGF0YSBvZiBmaWxlc1xuICogQHJldHVybnMge0JlZm9yZUJ1aWxkRm59XG4gKi9cbmV4cG9ydCBmdW5jdGlvbiB3aXRoQWNjZXNzTW9kZWwoYWNjZXNzTW9kZWw6IEZzQWNjZXNzTW9kZWwpOiBCZWZvcmVCdWlsZEZuIHtcbiAgcmV0dXJuIGFzeW5jIChfLCBjdHg6IEluaXRDb250ZXh0KSA9PiB7XG4gICAgaWYgKGN0eC5hbHJlYWR5U2V0QWNjZXNzTW9kZWwpIHtcbiAgICAgIHRocm93IG5ldyBFcnJvcihcbiAgICAgICAgYGFscmVhZHkgc2V0IHNvbWUgYXNzZXNzIG1vZGVsIGJlZm9yZTogJHtjdHguYWxyZWFkeVNldEFjY2Vzc01vZGVsLmNvbnN0cnVjdG9yPy5uYW1lfSgke2N0eC5hbHJlYWR5U2V0QWNjZXNzTW9kZWx9KWAsXG4gICAgICApO1xuICAgIH1cbiAgICBjdHguYWxyZWFkeVNldEFjY2Vzc01vZGVsID0gYWNjZXNzTW9kZWw7XG4gICAgcmV0dXJuIG5ldyBQcm9taXNlKHJlc29sdmUgPT4ge1xuICAgICAgY3R4LmJ1aWxkZXIuc2V0X2FjY2Vzc19tb2RlbChcbiAgICAgICAgYWNjZXNzTW9kZWwsXG4gICAgICAgIChwYXRoOiBzdHJpbmcpID0+IHtcbiAgICAgICAgICBjb25zdCBsYXN0TW9kaWZpZWQgPSBhY2Nlc3NNb2RlbC5nZXRNVGltZShwYXRoKTtcbiAgICAgICAgICBpZiAobGFzdE1vZGlmaWVkKSB7XG4gICAgICAgICAgICByZXR1cm4gbGFzdE1vZGlmaWVkLmdldFRpbWUoKTtcbiAgICAgICAgICB9XG4gICAgICAgICAgcmV0dXJuIDA7XG4gICAgICAgIH0sXG4gICAgICAgIChwYXRoOiBzdHJpbmcpID0+IHtcbiAgICAgICAgICByZXR1cm4gYWNjZXNzTW9kZWwuaXNGaWxlKHBhdGgpIHx8IGZhbHNlO1xuICAgICAgICB9LFxuICAgICAgICAocGF0aDogc3RyaW5nKSA9PiB7XG4gICAgICAgICAgcmV0dXJuIGFjY2Vzc01vZGVsLmdldFJlYWxQYXRoKHBhdGgpIHx8IHBhdGg7XG4gICAgICAgIH0sXG4gICAgICAgIChwYXRoOiBzdHJpbmcpID0+IHtcbiAgICAgICAgICByZXR1cm4gYWNjZXNzTW9kZWwucmVhZEFsbChwYXRoKTtcbiAgICAgICAgfSxcbiAgICAgICk7XG4gICAgICByZXNvbHZlKCk7XG4gICAgfSk7XG4gIH07XG59XG5cbi8qKlxuICogQGludGVybmFsIGJ1aWxkZXJcbiAqL1xudHlwZSBCdWlsZGVyID0gdHlwc3RSZW5kZXJlci5UeXBzdFJlbmRlcmVyQnVpbGRlciAmIHR5cHN0Q29tcGlsZXIuVHlwc3RDb21waWxlckJ1aWxkZXI7XG5cbi8qKlxuICogQGludGVybmFsIGJ1aWxkIGNvbnRleHRcbiAqL1xuaW50ZXJmYWNlIEluaXRDb250ZXh0IHtcbiAgcmVmOiB7XG4gICAgc2V0RmV0Y2hlcihmZXRjaGVyOiB0eXBlb2YgZmV0Y2gpOiB2b2lkO1xuICAgIGxvYWRGb250cyhidWlsZGVyOiBCdWlsZGVyLCBmb250czogKHN0cmluZyB8IFVpbnQ4QXJyYXkgfCBMYXp5Rm9udClbXSk6IFByb21pc2U8dm9pZD47XG4gIH07XG4gIGJ1aWxkZXI6IEJ1aWxkZXI7XG4gIGFscmVhZHlTZXRBY2Nlc3NNb2RlbDogYW55O1xufVxuXG4vLyB0b2RvOiBzZWFyY2ggYnJvd3NlclxuLy8gc2VhcmNoZXIuc2VhcmNoX2Jyb3dzZXIoKS5hd2FpdD87XG4iXX0=