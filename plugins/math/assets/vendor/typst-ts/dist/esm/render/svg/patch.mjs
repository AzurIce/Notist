/// Semantic attributes attached to SVG elements.
var TypstSvgAttrs;
(function (TypstSvgAttrs) {
    /// The data-tid attribute is used to identify the element.
    /// It is used to compare two elements.
    ///
    /// At most time, the data-tid is exactly their content hash.
    /// A disambiguation suffix is added when the content hash is not unique.
    TypstSvgAttrs["Tid"] = "data-tid";
    /// The data-reuse attribute is used to this element is reused from specified element.
    /// The attribute content is the data-tid of the element.
    TypstSvgAttrs["ReuseFrom"] = "data-reuse-from";
})(TypstSvgAttrs || (TypstSvgAttrs = {}));
/// Predicate that a xml element is a `<g>` element.
function isGElem(node) {
    return node.tagName === 'g';
}
/// Compare two elements by their data-tid attribute.
function equalElem(prev, next) {
    const prevDataTid = prev.getAttribute(TypstSvgAttrs.Tid);
    const nextDataTid = next.getAttribute(TypstSvgAttrs.Tid);
    return prevDataTid && prevDataTid === nextDataTid;
}
/// Interpret the transform between origin sequence and target sequence.
export function interpretTargetView(originChildren, targetChildren, tIsU = (_x) => true) {
    const availableOwnedResource = new Map();
    const targetView = [];
    for (let i = 0; i < originChildren.length; i++) {
        const prevChild = originChildren[i];
        if (!tIsU(prevChild)) {
            continue;
        }
        const data_tid = prevChild.getAttribute(TypstSvgAttrs.Tid);
        if (!data_tid) {
            targetView.push(['remove', i]);
            continue;
        }
        if (!availableOwnedResource.has(data_tid)) {
            availableOwnedResource.set(data_tid, [prevChild, []]);
        }
        availableOwnedResource.get(data_tid)[1].push(i);
    }
    const toPatch = [];
    for (let i = 0; i < targetChildren.length; i++) {
        const nextChild = targetChildren[i];
        if (!tIsU(nextChild)) {
            continue;
        }
        const nextDataTid = nextChild.getAttribute(TypstSvgAttrs.Tid);
        if (!nextDataTid) {
            throw new Error('not data tid for reusing g element for ' + nextDataTid);
        }
        const reuseTargetTid = nextChild.getAttribute(TypstSvgAttrs.ReuseFrom);
        if (!reuseTargetTid) {
            targetView.push(['append', nextChild]);
            continue;
        }
        if (!availableOwnedResource.has(reuseTargetTid)) {
            throw new Error('no available resource for reuse ' + reuseTargetTid);
        }
        const rsrc = availableOwnedResource.get(reuseTargetTid);
        const prevIdx = rsrc[1].shift();
        /// no available resource
        if (prevIdx === undefined) {
            /// clean one is reused directly
            if (nextDataTid === reuseTargetTid) {
                const clonedNode = rsrc[0].cloneNode(true);
                toPatch.push([clonedNode, nextChild]);
                targetView.push(['append', clonedNode]);
            }
            else {
                targetView.push(['append', nextChild]);
            }
            continue;
        }
        /// dirty one should be patched and reused
        toPatch.push([originChildren[prevIdx], nextChild]);
        targetView.push(['reuse', prevIdx]);
    }
    for (let [_, unusedIndices] of availableOwnedResource.values()) {
        for (let unused of unusedIndices) {
            targetView.push(['remove', unused]);
        }
    }
    return [targetView, toPatch];
}
/// Change a sequence of target view instructions to the origin ones.
/// Currently, it applies a greedy strategy.
/// + First, it applies all remove instructions.
/// + Then, it applies the swap ones.
/// + Finally, it inserts the extra elements.
///
/// Some better strategy would help and be implemented in future.
export function changeViewPerspective(originChildren, targetView, tIsU = (_x) => true) {
    const originView = [];
    /// see remove instructions
    let removeIndices = [];
    for (let inst of targetView) {
        if (inst[0] === 'remove') {
            removeIndices.push(inst[1]);
        }
    }
    removeIndices = removeIndices.sort((a, b) => a - b);
    const removeShift = [];
    /// apply remove instructions and get effect
    {
        let r = 0;
        for (let i = 0; i < removeIndices.length; i++) {
            while (r < removeIndices[i]) {
                removeShift.push(r - i);
                r++;
            }
            removeShift.push(undefined);
            originView.push(['remove', removeIndices[i] - i]);
            r++;
        }
        while (r <= originChildren.length) {
            removeShift.push(r - removeIndices.length);
            r++;
        }
    }
    // console.log(removeIndices, removeShift);
    /// get shift considering remove effects
    const getShift = (off) => {
        if (off >= removeShift.length || removeShift[off] === undefined) {
            throw new Error(`invalid offset ${off} for getShift ${removeShift}`);
        }
        return removeShift[off];
    };
    /// variables used by `interpretOriginView`
    /// scanning the target view
    let targetViewCursor = 0;
    /// the append effect
    let appendOffset = 0;
    /// converted append instructions.
    const swapIns = [];
    /// converted append instructions.
    const inserts = [];
    /// apply append and reuse instructions till the offset of origin sequence.
    const interpretOriginView = (off) => {
        // console.log(off, getShift(off));
        off = getShift(off);
        while (targetViewCursor < targetView.length) {
            let done = false;
            const inst = targetView[targetViewCursor];
            switch (inst[0]) {
                case 'append':
                    inserts.push(['insert', appendOffset, inst[1]]);
                    appendOffset++;
                    break;
                case 'reuse':
                    const target_off = getShift(inst[1]);
                    swapIns.push(target_off);
                    appendOffset++;
                    break;
                // case "remove":
                default:
                    break;
            }
            targetViewCursor++;
            if (done) {
                break;
            }
        }
    };
    /// scanning the origin view
    for (let off = 0; off < originChildren.length; off++) {
        const prevChild = originChildren[off];
        if (removeShift[off] === undefined) {
            continue;
        }
        // keep position of unpredictable elements
        if (!tIsU(prevChild)) {
            const target_off = getShift(off);
            swapIns.push(target_off);
            continue;
        }
        interpretOriginView(off);
    }
    interpretOriginView(originChildren.length);
    const simulated = [];
    for (let i = 0; i < swapIns.length; i++) {
        simulated.push(i);
    }
    for (let i = 0; i < swapIns.length; i++) {
        const off = swapIns[i];
        for (let j = 0; j < simulated.length; j++) {
            if (simulated[j] === off) {
                // console.log("swap_in", j, i, simulated);
                simulated.splice(j, 1);
                if (i <= j) {
                    simulated.splice(i, 0, off);
                }
                else {
                    simulated.splice(i + 1, 0, off);
                }
                if (j !== i) {
                    originView.push(['swap_in', i, j]);
                    // console.log("swap_in then", j, i, simulated);
                }
                break;
            }
        }
    }
    return [...originView, ...inserts];
}
function runOriginViewInstructions(prev, originView) {
    // console.log("interpreted origin view", originView);
    for (const [op, off, fr] of originView) {
        switch (op) {
            case 'insert':
                prev.insertBefore(fr, prev.children[off]);
                break;
            case 'swap_in':
                prev.insertBefore(prev.children[fr], prev.children[off]);
                break;
            case 'remove':
                prev.children[off].remove();
                break;
            default:
                throw new Error('unknown op ' + op);
        }
    }
}
/// End of View Interpretation
/// Begin of Recursive Svg Patch
/// Patch the `prev <svg>` in the DOM according to `next <svg>` from the backend.
export function patchRoot(prev, next) {
    /// Patch attributes
    patchAttributes(prev, next);
    /// Patch global svg resources
    patchSvgHeader(prev, next);
    /// Patch `<g>` children, call `reuseOrPatchElem` to patch.
    patchChildren(prev, next);
    return;
    function patchSvgHeader(prev, next) {
        for (let i = 0; i < 3; i++) {
            const prevChild = prev.children[i];
            const nextChild = next.children[i];
            // console.log("prev", prevChild);
            // console.log("next", nextChild);
            if (prevChild.tagName === 'defs') {
                if (prevChild.getAttribute('class') === 'glyph') {
                    // console.log("append glyphs:", nextChild.children, "to", prevChild);
                    prevChild.append(...nextChild.children);
                }
                else if (prevChild.getAttribute('class') === 'clip-path') {
                    // console.log("clip path: replace");
                    // todo: gc
                    prevChild.append(...nextChild.children);
                }
            }
            else if (prevChild.tagName === 'style' && nextChild.getAttribute('data-reuse') !== '1') {
                // console.log("replace extra style", prevChild, nextChild);
                // todo: gc
                if (nextChild.textContent) {
                    // todo: looks slow
                    // https://stackoverflow.com/questions/3326494/parsing-css-in-javascript-jquery
                    var doc = document.implementation.createHTMLDocument(''), styleElement = document.createElement('style');
                    styleElement.textContent = nextChild.textContent;
                    // the style will only be parsed once it is added to a document
                    doc.body.appendChild(styleElement);
                    const currentSvgSheet = prevChild.sheet;
                    const rulesToInsert = styleElement.sheet?.cssRules || [];
                    // console.log("rules to insert", currentSvgSheet, rulesToInsert);
                    for (const rule of rulesToInsert) {
                        currentSvgSheet.insertRule(rule.cssText);
                    }
                }
            }
        }
    }
}
/// apply attribute patches to the `prev <svg or g>` element
function patchAttributes(prev, next) {
    const prevAttrs = prev.attributes;
    const nextAttrs = next.attributes;
    if (prevAttrs.length === nextAttrs.length) {
        let same = true;
        for (let i = 0; i < prevAttrs.length; i++) {
            const prevAttr = prevAttrs[i];
            const nextAttr = nextAttrs.getNamedItem(prevAttr.name);
            if (nextAttr === null || prevAttr.value !== nextAttr.value) {
                same = false;
                break;
            }
        }
        if (same) {
            // console.log("same attributes, skip");
            return;
        }
    }
    // console.log("different attributes, replace");
    const removedAttrs = [];
    for (let i = 0; i < prevAttrs.length; i++) {
        removedAttrs.push(prevAttrs[i].name);
    }
    for (const attr of removedAttrs) {
        prev.removeAttribute(attr);
    }
    for (let i = 0; i < nextAttrs.length; i++) {
        prev.setAttribute(nextAttrs[i].name, nextAttrs[i].value);
    }
}
/// apply patches to the children sequence of `prev <svg or g>` in the DOM
function patchChildren(prev, next) {
    const [targetView, toPatch] = interpretTargetView(prev.children, next.children, isGElem);
    for (let [prevChild, nextChild] of toPatch) {
        reuseOrPatchElem(prevChild, nextChild);
    }
    // console.log("interpreted target view", targetView);
    const originView = changeViewPerspective(prev.children, targetView, isGElem);
    runOriginViewInstructions(prev, originView);
}
/// Replace the `prev` element with `next` element.
/// Return true if the `prev` element is reused.
/// Return false if the `prev` element is replaced.
function reuseOrPatchElem(prev, next) {
    const canReuse = equalElem(prev, next);
    /// Even if the element is reused, we still need to replace its attributes.
    next.removeAttribute(TypstSvgAttrs.ReuseFrom);
    patchAttributes(prev, next);
    if (canReuse) {
        return true /* reused */;
    }
    /// Hard replace elements that is not a `<g>` element.
    replaceNonSVGElements(prev, next);
    /// Patch `<g>` children, will call `reuseOrPatchElem` again.
    patchChildren(prev, next);
    return false /* reused */;
    function replaceNonSVGElements(prev, next) {
        const removedIndices = [];
        for (let i = 0; i < prev.children.length; i++) {
            const prevChild = prev.children[i];
            if (!isGElem(prevChild)) {
                removedIndices.push(i);
            }
        }
        for (const index of removedIndices.reverse()) {
            prev.children[index].remove();
        }
        for (let i = 0; i < next.children.length; i++) {
            const nextChild = next.children[i];
            if (!isGElem(nextChild)) {
                prev.appendChild(nextChild.cloneNode(true));
            }
        }
    }
}
/// End of Recursive Svg Patch
//# sourceMappingURL=data:application/json;base64,eyJ2ZXJzaW9uIjozLCJmaWxlIjoicGF0Y2gubWpzIiwic291cmNlUm9vdCI6IiIsInNvdXJjZXMiOlsiLi4vLi4vLi4vLi4vc3JjL3JlbmRlci9zdmcvcGF0Y2gubXRzIl0sIm5hbWVzIjpbXSwibWFwcGluZ3MiOiJBQU9BLGlEQUFpRDtBQUNqRCxJQUFXLGFBV1Y7QUFYRCxXQUFXLGFBQWE7SUFDdEIsMkRBQTJEO0lBQzNELHVDQUF1QztJQUN2QyxHQUFHO0lBQ0gsNkRBQTZEO0lBQzdELHlFQUF5RTtJQUN6RSxpQ0FBZ0IsQ0FBQTtJQUVoQixzRkFBc0Y7SUFDdEYseURBQXlEO0lBQ3pELDhDQUE2QixDQUFBO0FBQy9CLENBQUMsRUFYVSxhQUFhLEtBQWIsYUFBYSxRQVd2QjtBQUVELG9EQUFvRDtBQUNwRCxTQUFTLE9BQU8sQ0FBQyxJQUFhO0lBQzVCLE9BQU8sSUFBSSxDQUFDLE9BQU8sS0FBSyxHQUFHLENBQUM7QUFDOUIsQ0FBQztBQUVELHFEQUFxRDtBQUNyRCxTQUFTLFNBQVMsQ0FBQyxJQUFpQixFQUFFLElBQWlCO0lBQ3JELE1BQU0sV0FBVyxHQUFHLElBQUksQ0FBQyxZQUFZLENBQUMsYUFBYSxDQUFDLEdBQUcsQ0FBQyxDQUFDO0lBQ3pELE1BQU0sV0FBVyxHQUFHLElBQUksQ0FBQyxZQUFZLENBQUMsYUFBYSxDQUFDLEdBQUcsQ0FBQyxDQUFDO0lBQ3pELE9BQU8sV0FBVyxJQUFJLFdBQVcsS0FBSyxXQUFXLENBQUM7QUFDcEQsQ0FBQztBQXFDRCx3RUFBd0U7QUFDeEUsTUFBTSxVQUFVLG1CQUFtQixDQUNqQyxjQUFtQixFQUNuQixjQUFtQixFQUNuQixPQUFPLENBQUMsRUFBSyxFQUFXLEVBQUUsQ0FBQyxJQUFJO0lBRS9CLE1BQU0sc0JBQXNCLEdBQUcsSUFBSSxHQUFHLEVBQXlCLENBQUM7SUFDaEUsTUFBTSxVQUFVLEdBQStCLEVBQUUsQ0FBQztJQUVsRCxLQUFLLElBQUksQ0FBQyxHQUFHLENBQUMsRUFBRSxDQUFDLEdBQUcsY0FBYyxDQUFDLE1BQU0sRUFBRSxDQUFDLEVB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QUFTLENBQUMsTUFBTSxLQUFLLFNBQVMsQ0FBQyxNQUFNLEVBQUUsQ0FBQztRQUMxQyxJQUFJLElBQUksR0FBRyxJQUFJLENBQUM7UUFDaEIsS0FBSyxJQUFJLENBQUMsR0FBRyxDQUFDLEVBQUUsQ0FBQyxHQUFHLFNBQVMsQ0FBQyxNQUFNLEVBQUUsQ0FBQyxFQUFFLEVBQUUsQ0FBQztZQUMxQyxNQUFNLFFBQVEsR0FBRyxTQUFTLENBQUMsQ0FBQyxDQUFDLENBQUM7WUFDOUIsTUFBTSxRQUFRLEdBQUcsU0FBUyxDQUFDLFlBQVksQ0FBQyxRQUFRLENBQUMsSUFBSSxDQUFDLENBQUM7WUFDdkQsSUFBSSxRQUFRLEtBQUssSUFBSSxJQUFJLFFBQVEsQ0FBQyxLQUFLLEtBQUssUUFBUSxDQUFDLEtBQUssRUFBRSxDQUFDO2dCQUMzRCxJQUFJLEdBQUcsS0FBSyxDQUFDO2dCQUNiLE1BQU07WUFDUixDQUFDO1FBQ0gsQ0FBQztRQUVELElBQUksSUFBSSxFQUFFLENBQUM7WUFDVCx3Q0FBd0M7WUFDeEMsT0FBTztRQUNULENBQUM7SUFDSCxDQUFDO0lBQ0QsZ0RBQWdEO0lBRWhELE1BQU0sWUFBWSxHQUFhLEVBQUUsQ0FBQztJQUVsQyxLQUFLLElBQUksQ0FBQyxHQUFHLENBQUMsRUFBRSxDQUFDLEdBQUcsU0FBUyxDQUFDLE1BQU0sRUFBRSxDQUFDLEVBQUUsRUFBRSxDQUFDO1FBQzFDLFlBQVksQ0FBQyxJQUFJLENBQUMsU0FBUyxDQUFDLENBQUMsQ0FBQyxDQUFDLElBQUksQ0FBQyxDQUFDO0lBQ3ZDLENBQUM7SUFFRCxLQUFLLE1BQU0sSUFBSSxJQUFJLFlBQVksRUFBRSxDQUFDO1FBQ2hDLElBQUksQ0FBQyxlQUFlLENBQUMsSUFBSSxDQUFDLENBQUM7SUFDN0IsQ0FBQztJQUVELEtBQUssSUFBSSxDQUFDLEdBQUcsQ0FBQyxFQUFFLENBQUMsR0FBRyxTQUFTLENBQUMsTUFBTSxFQUFFLENBQUMsRUFBRSxFQUFFLENBQUM7UUFDMUMsSUFBSSxDQUFDLFlBQVksQ0FBQyxTQUFTLENBQUMsQ0FBQyxDQUFDLENBQUMsSUFBSSxFQUFFLFNBQVMsQ0FBQyxDQUFDLENBQUMsQ0FBQyxLQUFLLENBQUMsQ0FBQztJQUMzRCxDQUFDO0FBQ0gsQ0FBQztBQUVELDBFQUEwRTtBQUMxRSxTQUFTLGFBQWEsQ0FBQyxJQUFhLEVBQUUsSUFBYTtJQUNqRCxNQUFNLENBQUMsVUFBVSxFQUFFLE9BQU8sQ0FBQyxHQUFHLG1CQUFtQixDQUMvQyxJQUFJLENBQUMsUUFBb0MsRUFDekMsSUFBSSxDQUFDLFFBQW9DLEVBQ3pDLE9BQU8sQ0FDUixDQUFDO0lBRUYsS0FBSyxJQUFJLENBQUMsU0FBUyxFQUFFLFNBQVMsQ0FBQyxJQUFJLE9BQU8sRUFBRSxDQUFDO1FBQzNDLGdCQUFnQixDQUFDLFNBQVMsRUFBRSxTQUFTLENBQUMsQ0FBQztJQUN6QyxDQUFDO0lBRUQsc0RBQXNEO0lBRXRELE1BQU0sVUFBVSxHQUFHLHFCQUFxQixDQUN0QyxJQUFJLENBQUMsUUFBb0MsRUFDekMsVUFBVSxFQUNWLE9BQU8sQ0FDUixDQUFDO0lBRUYseUJBQXlCLENBQUMsSUFBSSxFQUFFLFVBQVUsQ0FBQyxDQUFDO0FBQzlDLENBQUM7QUFFRCxtREFBbUQ7QUFDbkQsZ0RBQWdEO0FBQ2hELG1EQUFtRDtBQUNuRCxTQUFTLGdCQUFnQixDQUFDLElBQWlCLEVBQUUsSUFBaUI7SUFDNUQsTUFBTSxRQUFRLEdBQUcsU0FBUyxDQUFDLElBQUksRUFBRSxJQUFJLENBQUMsQ0FBQztJQUV2QywyRUFBMkU7SUFDM0UsSUFBSSxDQUFDLGVBQWUsQ0FBQyxhQUFhLENBQUMsU0FBUyxDQUFDLENBQUM7SUFDOUMsZUFBZSxDQUFDLElBQUksRUFBRSxJQUFJLENBQUMsQ0FBQztJQUU1QixJQUFJLFFBQVEsRUFBRSxDQUFDO1FBQ2IsT0FBTyxJQUFJLENBQUMsWUFBWSxDQUFDO0lBQzNCLENBQUM7SUFFRCxzREFBc0Q7SUFDdEQscUJBQXFCLENBQUMsSUFBSSxFQUFFLElBQUksQ0FBQyxDQUFDO0lBQ2xDLDZEQUE2RDtJQUM3RCxhQUFhLENBQUMsSUFBSSxFQUFFLElBQUksQ0FBQyxDQUFDO0lBQzFCLE9BQU8sS0FBSyxDQUFDLFlBQVksQ0FBQztJQUUxQixTQUFTLHFCQUFxQixDQUFDLElBQWEsRUFBRSxJQUFhO1FBQ3pELE1BQU0sY0FBYyxHQUFhLEVBQUUsQ0FBQztRQUNwQyxLQUFLLElBQUksQ0FBQyxHQUFHLENBQUMsRUFBRSxDQUFDLEdBQUcsSUFBSSxDQUFDLFFBQVEsQ0FBQyxNQUFNLEVBQUUsQ0FBQyxFQUFFLEVBQUUsQ0FBQztZQUM5QyxNQUFNLFNBQVMsR0FBRyxJQUFJLENBQUMsUUFBUSxDQUFDLENBQUMsQ0FBQyxDQUFDO1lBQ25DLElBQUksQ0FBQyxPQUFPLENBQUMsU0FBUyxDQUFDLEVBQUUsQ0FBQztnQkFDeEIsY0FBYyxDQUFDLElBQUksQ0FBQyxDQUFDLENBQUMsQ0FBQztZQUN6QixDQUFDO1FBQ0gsQ0FBQztRQUVELEtBQUssTUFBTSxLQUFLLElBQUksY0FBYyxDQUFDLE9BQU8sRUFBRSxFQUFFLENBQUM7WUFDN0MsSUFBSSxDQUFDLFFBQVEsQ0FBQyxLQUFLLENBQUMsQ0FBQyxNQUFNLEVBQUUsQ0FBQztRQUNoQyxDQUFDO1FBRUQsS0FBSyxJQUFJLENBQUMsR0FBRyxDQUFDLEVBQUUsQ0FBQyxHQUFHLElBQUksQ0FBQyxRQUFRLENBQUMsTUFBTSxFQUFFLENBQUMsRUFBRSxFQUFFLENBQUM7WUFDOUMsTUFBTSxTQUFTLEdBQUcsSUFBSSxDQUFDLFFBQVEsQ0FBQyxDQUFDLENBQUMsQ0FBQztZQUNuQyxJQUFJLENBQUMsT0FBTyxDQUFDLFNBQVMsQ0FBQyxFQUFFLENBQUM7Z0JBQ3hCLElBQUksQ0FBQyxXQUFXLENBQUMsU0FBUyxDQUFDLFNBQVMsQ0FBQyxJQUFJLENBQUMsQ0FBQyxDQUFDO1lBQzlDLENBQUM7UUFDSCxDQUFDO0lBQ0gsQ0FBQztBQUNILENBQUM7QUFFRCw4QkFBOEIiLCJzb3VyY2VzQ29udGVudCI6WyIvLy8gVGhlIEVsZW1lbnRDaGlsZHJlbiByZXByZXNlbnRzIGFuIG9iamVjdCBvZiBhIGxpc3Qgb2Ygbm9kZXMuXG5leHBvcnQgaW50ZXJmYWNlIEVsZW1lbnRDaGlsZHJlbiB7XG4gIHRhZ05hbWU6IHN0cmluZztcbiAgZ2V0QXR0cmlidXRlKG5hbWU6IHN0cmluZyk6IHN0cmluZyB8IG51bGw7XG4gIGNsb25lTm9kZShkZWVwOiBib29sZWFuKTogdW5rbm93bjtcbn1cblxuLy8vIFNlbWFudGljIGF0dHJpYnV0ZXMgYXR0YWNoZWQgdG8gU1ZHIGVsZW1lbnRzLlxuY29uc3QgZW51bSBUeXBzdFN2Z0F0dHJzIHtcbiAgLy8vIFRoZSBkYXRhLXRpZCBhdHRyaWJ1dGUgaXMgdXNlZCB0byBpZGVudGlmeSB0aGUgZWxlbWVudC5cbiAgLy8vIEl0IGlzIHVzZWQgdG8gY29tcGFyZSB0d28gZWxlbWVudHMuXG4gIC8vL1xuICAvLy8gQXQgbW9zdCB0aW1lLCB0aGUgZGF0YS10aWQgaXMgZXhhY3RseSB0aGVpciBjb250ZW50IGhhc2guXG4gIC8vLyBBIGRpc2FtYmlndWF0aW9uIHN1ZmZpeCBpcyBhZGRlZCB3aGVuIHRoZSBjb250ZW50IGhhc2ggaXMgbm90IHVuaXF1ZS5cbiAgVGlkID0gJ2RhdGEtdGlkJyxcblxuICAvLy8gVGhlIGRhdGEtcmV1c2UgYXR0cmlidXRlIGlzIHVzZWQgdG8gdGhpcyBlbGVtZW50IGlzIHJldXNlZCBmcm9tIHNwZWNpZmllZCBlbGVtZW50LlxuICAvLy8gVGhlIGF0dHJpYnV0ZSBjb250ZW50IGlzIHRoZSBkYXRhLXRpZCBvZiB0aGUgZWxlbWVudC5cbiAgUmV1c2VGcm9tID0gJ2RhdGEtcmV1c2UtZnJvbScsXG59XG5cbi8vLyBQcmVkaWNhdGUgdGhhdCBhIHhtbCBlbGVtZW50IGlzIGEgYDxnPmAgZWxlbWVudC5cbmZ1bmN0aW9uIGlzR0VsZW0obm9kZTogRWxlbWVudCk6IG5vZGUgaXMgU1ZHR0VsZW1lbnQge1xuICByZXR1cm4gbm9kZS50YWdOYW1lID09PSAnZyc7XG59XG5cbi8vLyBDb21wYXJlIHR3byBlbGVtZW50cyBieSB0aGVpciBkYXRhLXRpZCBhdHRyaWJ1dGUuXG5mdW5jdGlvbiBlcXVhbEVsZW0ocHJldjogU1ZHR0VsZW1lbnQsIG5leHQ6IFNWR0dFbGVtZW50KSB7XG4gIGNvbnN0IHByZXZEYXRhVGlkID0gcHJldi5nZXRBdHRyaWJ1dGUoVHlwc3RTdmdBdHRycy5UaWQpO1xuICBjb25zdCBuZXh0RGF0YVRpZCA9IG5leHQuZ2V0QXR0cmlidXRlKFR5cHN0U3ZnQXR0cnMuVGlkKTtcbiAgcmV0dXJuIHByZXZEYXRhVGlkICYmIHByZXZEYXRhVGlkID09PSBuZXh0RGF0YVRpZDtcbn1cblxuLy8vIEJlZ2luIG9mIFZpZXcgSW50ZXJwcmV0YXRpb25cbi8vL1xuLy8vICMjIyBWaWV3XG4vLy9cbi8vLyBUaGUgdmlldyBpcyBkZWZpbmVkIGFzIGEgc3RydWN0dXJlIHRvIGRlc2NyaWJlIHRoZSBzdGF0ZSBvZiBhIHNlcXVlbmNlLFxuLy8vIFdoaWxlIHRoZSB2aWV3IGluc3RydWN0aW9ucyBkZXNjcmliZSB0aGUgY2hhbmdlIG9mIHZpZXcuXG4vLy8gICBUaGF0IGlzLCBHaXZlbiBWLCBWJywgViBiZWNvbWVzIFYnIGFmdGVyIGFwcGx5aW5nIHZpZXcgaW5zdHJ1Y3Rpb25zIGluIG9yZGVyLlxuLy8vIFdlIGNhbGwgVicgdGhlIHRhcmdldCB2aWV3LCBhbmQgVicgdGhlIG9yaWdpbiB2aWV3LlxuLy8vXG4vLy8gIyMjIFZpZXcgSW5zdHJ1Y3Rpb25cbi8vL1xuLy8vIEludHJvZHVjZWQgdGhlIHRhcmdldC9vcmlnaW4gdmlldywgdGhlcmUgYXJlIHR3byB0eXBlIG9mIGluc3RydWN0aW9ucyB0byBub3RlOlxuXG4vLy8gVGhlIHRhcmdldCB2aWV3IGluc3RydWN0aW9ucyBhcmUgZ2VuZXJhdGVkIGJ5XG4vLy8gICBjb21wYXJpbmcgd2l0aCB0aGUgdGFyZ2V0IHZpZXcgYW5kIG9yaWdpbiB2aWV3LlxuLy8vIFRoZSBpbnN0cnVjdGlvbiBzZXF1ZW5jZSBzcGVjaWZ5IGhvdyB3ZSBjYW4gZ2VuZXJhdGUgYSB2aWV3IHdpdGggY29uZGl0aW9uczpcbi8vLyArIEl0IGlzIGdlbmVyYXRlZCBmcm9tIGEgZW1wdHkgc2VxdWVuY2UuXG4vLy8gKyBJdCBjYW4gdXRpbGl6ZSBhIHNldCBvZiBlbGVtZW50cyBhcyByZXNvdXJjZXMuXG4vLy9cbi8vLyBFeGFtcGxlMTogcmVzb3VyY2U6W10gLT4gPGFwcGVuZCB0MT4gLT4gW3QxXVxuLy8vIEV4YW1wbGUyOiByZXNvdXJjZTpbbzFdIC0+IDxyZXVzZSBvMT4gLT4gW28xXVxuLy8vIEV4YW1wbGUzOiByZXNvdXJjZTpbbzFdIC0+IDxyZXVzZSBvMT4gPHJldXNlIG8xPiAtPiBbbzEsIG8xXVxuLy8vIEV4YW1wbGU0OiByZXNvdXJjZTpbbzEsIG8yXSAtPiA8cmV1c2UgbzE+IDxhcHBlbmQgdDE+IC0+IFtvMSwgdDFdXG4vLy9cbi8vLyBUbyByZW1vdmUgdW51c2VkIHJlc291cmNlcywgQW4gZXh0cmEgcmVtb3ZlIGluc3QgY2FuIHJlbW92ZSBhIHNwZWNpZnkgZWxlbWVudFxuLy8vXG4vLy8gRXhhbXBsZTU6IHJlc291cmNlOltvMSwgbzJdIC0+IDxyZXVzZSBvMT4gPGFwcGVuZCB0MT4gPHJlbW92ZSBvMj4gLT4gW28xLCB0MV0gYW5kIHJlbW92ZSBvMlxuZXhwb3J0IHR5cGUgVGFyZ2V0Vmlld0luc3RydWN0aW9uPFQ+ID0gWydhcHBlbmQnLCBUXSB8IFsncmV1c2UnLCBudW1iZXJdIHwgWydyZW1vdmUnLCBudW1iZXJdO1xuXG4vLy8gVGhlIHJlY3Vyc2l2ZSBwYXRjaCBvcGVyYXRpb24gbXVzdCBiZSBhcHBsaWVkIHRvIHRoaXMgdHdvIGVsZW1lbnQuXG5leHBvcnQgdHlwZSBQYXRjaFBhaXI8VD4gPSBbVCAvKiBvcmlnaW4gKi8sIFQgLyogdGFyZ2V0ICovXTtcblxuLy8vIEludGVycHJldGVkIHJlc3VsdCBmb3IgdHJhbnNmb3JtaW5nIG9yaWdpbiBzZXF1ZW5jZSB0byB0YXJnZXQgc2VxdWVuY2UuXG5leHBvcnQgdHlwZSBWaWV3VHJhbnNmb3JtPFU+ID0gW1RhcmdldFZpZXdJbnN0cnVjdGlvbjxVPltdLCBQYXRjaFBhaXI8VT5bXV07XG5cbi8vLyBJbnRlcnByZXQgdGhlIHRyYW5zZm9ybSBiZXR3ZWVuIG9yaWdpbiBzZXF1ZW5jZSBhbmQgdGFyZ2V0IHNlcXVlbmNlLlxuZXhwb3J0IGZ1bmN0aW9uIGludGVycHJldFRhcmdldFZpZXc8VCBleHRlbmRzIEVsZW1lbnRDaGlsZHJlbiwgVSBleHRlbmRzIFQgPSBUPihcbiAgb3JpZ2luQ2hpbGRyZW46IFRbXSxcbiAgdGFyZ2V0Q2hpbGRyZW46IFRbXSxcbiAgdElzVSA9IChfeDogVCk6IF94IGlzIFUgPT4gdHJ1ZSxcbik6IFZpZXdUcmFuc2Zvcm08VT4ge1xuICBjb25zdCBhdmFpbGFibGVPd25lZFJlc291cmNlID0gbmV3IE1hcDxzdHJpbmcsIFtULCBudW1iZXJbXV0+KCk7XG4gIGNvbnN0IHRhcmdldFZpZXc6IFRhcmdldFZpZXdJbnN0cnVjdGlvbjxVPltdID0gW107XG5cbiAgZm9yIChsZXQgaSA9IDA7IGkgPCBvcmlnaW5DaGlsZHJlbi5sZW5ndGg7IGkrKykge1xuICAgIGNvbnN0IHByZXZDaGlsZCA9IG9yaWdpbkNoaWxkcmVuW2ldO1xuICAgIGlmICghdElzVShwcmV2Q2hpbGQpKSB7XG4gICAgICBjb250aW51ZTtcbiAgICB9XG5cbiAgICBjb25zdCBkYXRhX3RpZCA9IHByZXZDaGlsZC5nZXRBdHRyaWJ1dGUoVHlwc3RTdmdBdHRycy5UaWQpO1xuICAgIGlmICghZGF0YV90aWQpIHtcbiAgICAgIHRhcmdldFZpZXcucHVzaChbJ3JlbW92ZScsIGldKTtcbiAgICAgIGNvbnRpbnVlO1xuICAgIH1cblxuICAgIGlmICghYXZhaWxhYmxlT3duZWRSZXNvdXJjZS5oYXMoZGF0YV90aWQpKSB7XG4gICAgICBhdmFpbGFibGVPd25lZFJlc291cmNlLnNldChkYXRhX3RpZCwgW3ByZXZDaGlsZCwgW11dKTtcbiAgICB9XG4gICAgYXZhaWxhYmxlT3duZWRSZXNvdXJjZS5nZXQoZGF0YV90aWQpIVsxXS5wdXNoKGkpO1xuICB9XG5cbiAgY29uc3QgdG9QYXRjaDogW1UsIFVdW10gPSBbXTtcblxuICBmb3IgKGxldCBpID0gMDsgaSA8IHRhcmdldENoaWxkcmVuLmxlbmd0aDsgaSsrKSB7XG4gICAgY29uc3QgbmV4dENoaWxkID0gdGFyZ2V0Q2hpbGRyZW5baV07XG4gICAgaWYgKCF0SXNVKG5leHRDaGlsZCkpIHtcbiAgICAgIGNvbnRpbnVlO1xuICAgIH1cblxuICAgIGNvbnN0IG5leHREYXRhVGlkID0gbmV4dENoaWxkLmdldEF0dHJpYnV0ZShUeXBzdFN2Z0F0dHJzLlRpZCk7XG4gICAgaWYgKCFuZXh0RGF0YVRpZCkge1xuICAgICAgdGhyb3cgbmV3IEVycm9yKCdub3QgZGF0YSB0aWQgZm9yIHJldXNpbmcgZyBlbGVtZW50IGZvciAnICsgbmV4dERhdGFUaWQpO1xuICAgIH1cblxuICAgIGNvbnN0IHJldXNlVGFyZ2V0VGlkID0gbmV4dENoaWxkLmdldEF0dHJpYnV0ZShUeXBzdFN2Z0F0dHJzLlJldXNlRnJvbSk7XG4gICAgaWYgKCFyZXVzZVRhcmdldFRpZCkge1xuICAgICAgdGFyZ2V0Vmlldy5wdXNoKFsnYXBwZW5kJywgbmV4dENoaWxkXSk7XG4gICAgICBjb250aW51ZTtcbiAgICB9XG4gICAgaWYgKCFhdmFpbGFibGVPd25lZFJlc291cmNlLmhhcyhyZXVzZVRhcmdldFRpZCkpIHtcbiAgICAgIHRocm93IG5ldyBFcnJvcignbm8gYXZhaWxhYmxlIHJlc291cmNlIGZvciByZXVzZSAnICsgcmV1c2VUYXJnZXRUaWQpO1xuICAgIH1cblxuICAgIGNvbnN0IHJzcmMgPSBhdmFpbGFibGVPd25lZFJlc291cmNlLmdldChyZXVzZVRhcmdldFRpZCkhO1xuICAgIGNvbnN0IHByZXZJZHggPSByc3JjWzFdLnNoaWZ0KCk7XG5cbiAgICAvLy8gbm8gYXZhaWxhYmxlIHJlc291cmNlXG4gICAgaWYgKHByZXZJZHggPT09IHVuZGVmaW5lZCkge1xuICAgICAgLy8vIGNsZWFuIG9uZSBpcyByZXVzZWQgZGlyZWN0bHlcbiAgICAgIGlmIChuZXh0RGF0YVRpZCA9PT0gcmV1c2VUYXJnZXRUaWQpIHtcbiAgICAgICAgY29uc3QgY2xvbmVkTm9kZSA9IHJzcmNbMF0uY2xvbmVOb2RlKHRydWUpIGFzIFU7XG4gICAgICAgIHRvUGF0Y2gucHVzaChbY2xvbmVkTm9kZSwgbmV4dENoaWxkXSk7XG4gICAgICAgIHRhcmdldFZpZXcucHVzaChbJ2FwcGVuZCcsIGNsb25lZE5vZGVdKTtcbiAgICAgIH0gZWxzZSB7XG4gICAgICAgIHRhcmdldFZpZXcucHVzaChbJ2FwcGVuZCcsIG5leHRDaGlsZF0pO1xuICAgICAgfVxuICAgICAgY29udGludWU7XG4gICAgfVxuXG4gICAgLy8vIGRpcnR5IG9uZSBzaG91bGQgYmUgcGF0Y2hlZCBhbmQgcmV1c2VkXG4gICAgdG9QYXRjaC5wdXNoKFtvcmlnaW5DaGlsZHJlbltwcmV2SWR4XSBhcyBVLCBuZXh0Q2hpbGRdKTtcbiAgICB0YXJnZXRWaWV3LnB1c2goWydyZXVzZScsIHByZXZJZHhdKTtcbiAgfVxuXG4gIGZvciAobGV0IFtfLCB1bnVzZWRJbmRpY2VzXSBvZiBhdmFpbGFibGVPd25lZFJlc291cmNlLnZhbHVlcygpKSB7XG4gICAgZm9yIChsZXQgdW51c2VkIG9mIHVudXNlZEluZGljZXMpIHtcbiAgICAgIHRhcmdldFZpZXcucHVzaChbJ3JlbW92ZScsIHVudXNlZF0pO1xuICAgIH1cbiAgfVxuXG4gIHJldHVybiBbdGFyZ2V0VmlldywgdG9QYXRjaF07XG59XG5cbi8vLyBUaGUgb3JpZ2luIHZpZXcgaW5zdHJ1Y3Rpb25zIGFyZSBzZW1hbnRpYy1wcmVzZXJ2ZWQgdG8gdGhlIHRhcmdldCBvbmVzLlxuLy8vIFRoZSBtYWpvciBkaWZmZXJlbmNlcyBiZXR3ZWVuIHRoZSBvcmlnaW4gb25lcyBhbmQgdGFyZ2V0IG9uZXMgYXJlIHRoYXQ6XG4vLy8gKyBJdCBjYW4gYmUgZWFzaWx5IGFwcGxpZWQgdG8gYSBET00gc2VxdWVuY2Vcbi8vLyArIEl0IGhhcyBiZXR0ZXIgcGVyZm9ybWFuY2UuXG4vLy9cbi8vLyBFeGFtcGxlMTogZG9tOltvMCwgbzFdIC0+IDxpbnNlcnQgYXQgMSwgdDE+IC0+IFtvMCwgdDEsIG8xXVxuLy8vIEV4YW1wbGUyOiBkb206W28wLCBvMSwgbzIsIG8zXSAtPiA8c3dhcF9pbiBhdCAyLCAwPiAtPiBbbzEsIG8wLCBvMiwgbzNdXG4vLy8gRXhhbXBsZTM6IGRvbTpbbzAsIG8xLCBvMiwgbzMsIG80XSAtPiA8c3dhcF9pbiBhdCAzLCAwPiAtPiBbbzEsIG8yLCBvMCwgbzNdXG4vLy8gRXhhbXBsZTQ6IGRvbTpbbzAsIG8xLCBvMl0gLT4gPHJlbW92ZSBhdCAxPiAtPiBbbzAsIG8yXVxuZXhwb3J0IHR5cGUgT3JpZ2luVmlld0luc3RydWN0aW9uPFQ+ID1cbiAgfCBbJ2luc2VydCcsIG51bWJlciwgVF1cbiAgfCBbJ3N3YXBfaW4nLCBudW1iZXIsIG51bWJlcl1cbiAgfCBbJ3JlbW92ZScsIG51bWJlcl07XG5cbi8vLyBDaGFuZ2UgYSBzZXF1ZW5jZSBvZiB0YXJnZXQgdmlldyBpbnN0cnVjdGlvbnMgdG8gdGhlIG9yaWdpbiBvbmVzLlxuLy8vIEN1cnJlbnRseSwgaXQgYXBwbGllcyBhIGdyZWVkeSBzdHJhdGVneS5cbi8vLyArIEZpcnN0LCBpdCBhcHBsaWVzIGFsbCByZW1vdmUgaW5zdHJ1Y3Rpb25zLlxuLy8vICsgVGhlbiwgaXQgYXBwbGllcyB0aGUgc3dhcCBvbmVzLlxuLy8vICsgRmluYWxseSwgaXQgaW5zZXJ0cyB0aGUgZXh0cmEgZWxlbWVudHMuXG4vLy9cbi8vLyBTb21lIGJldHRlciBzdHJhdGVneSB3b3VsZCBoZWxwIGFuZCBiZSBpbXBsZW1lbnRlZCBpbiBmdXR1cmUuXG5leHBvcnQgZnVuY3Rpb24gY2hhbmdlVmlld1BlcnNwZWN0aXZlPFQgZXh0ZW5kcyBFbGVtZW50Q2hpbGRyZW4sIFUgZXh0ZW5kcyBUID0gVD4oXG4gIG9yaWdpbkNoaWxkcmVuOiBUW10sXG4gIHRhcmdldFZpZXc6IFRhcmdldFZpZXdJbnN0cnVjdGlvbjxVPltdLFxuICB0SXNVID0gKF94OiBUKTogX3ggaXMgVSA9PiB0cnVlLFxuKTogT3JpZ2luVmlld0luc3RydWN0aW9uPFU+W10ge1xuICBjb25zdCBvcmlnaW5WaWV3OiBPcmlnaW5WaWV3SW5zdHJ1Y3Rpb248VT5bXSA9IFtdO1xuXG4gIC8vLyBzZWUgcmVtb3ZlIGluc3RydWN0aW9uc1xuICBsZXQgcmVtb3ZlSW5kaWNlczogbnVtYmVyW10gPSBbXTtcbiAgZm9yIChsZXQgaW5zdCBvZiB0YXJnZXRWaWV3KSB7XG4gICAgaWYgKGluc3RbMF0gPT09ICdyZW1vdmUnKSB7XG4gICAgICByZW1vdmVJbmRpY2VzLnB1c2goaW5zdFsxXSk7XG4gICAgfVxuICB9XG4gIHJlbW92ZUluZGljZXMgPSByZW1vdmVJbmRpY2VzLnNvcnQoKGEsIGIpID0+IGEgLSBiKTtcbiAgY29uc3QgcmVtb3ZlU2hpZnQ6IChudW1iZXIgfCB1bmRlZmluZWQpW10gPSBbXTtcblxuICAvLy8gYXBwbHkgcmVtb3ZlIGluc3RydWN0aW9ucyBhbmQgZ2V0IGVmZmVjdFxuICB7XG4gICAgbGV0IHIgPSAwO1xuICAgIGZvciAobGV0IGkgPSAwOyBpIDwgcmVtb3ZlSW5kaWNlcy5sZW5ndGg7IGkrKykge1xuICAgICAgd2hpbGUgKHIgPCByZW1vdmVJbmRpY2VzW2ldKSB7XG4gICAgICAgIHJlbW92ZVNoaWZ0LnB1c2gociAtIGkpO1xuICAgICAgICByKys7XG4gICAgICB9XG4gICAgICByZW1vdmVTaGlmdC5wdXNoKHVuZGVmaW5lZCk7XG4gICAgICBvcmlnaW5WaWV3LnB1c2goWydyZW1vdmUnLCByZW1vdmVJbmRpY2VzW2ldIC0gaV0pO1xuICAgICAgcisrO1xuICAgIH1cbiAgICB3aGlsZSAociA8PSBvcmlnaW5DaGlsZHJlbi5sZW5ndGgpIHtcbiAgICAgIHJlbW92ZVNoaWZ0LnB1c2gociAtIHJlbW92ZUluZGljZXMubGVuZ3RoKTtcbiAgICAgIHIrKztcbiAgICB9XG4gIH1cbiAgLy8gY29uc29sZS5sb2cocmVtb3ZlSW5kaWNlcywgcmVtb3ZlU2hpZnQpO1xuICAvLy8gZ2V0IHNoaWZ0IGNvbnNpZGVyaW5nIHJlbW92ZSBlZmZlY3RzXG4gIGNvbnN0IGdldFNoaWZ0ID0gKG9mZjogbnVtYmVyKSA9PiB7XG4gICAgaWYgKG9mZiA+PSByZW1vdmVTaGlmdC5sZW5ndGggfHwgcmVtb3ZlU2hpZnRbb2ZmXSA9PT0gdW5kZWZpbmVkKSB7XG4gICAgICB0aHJvdyBuZXcgRXJyb3IoYGludmFsaWQgb2Zmc2V0ICR7b2ZmfSBmb3IgZ2V0U2hpZnQgJHtyZW1vdmVTaGlmdH1gKTtcbiAgICB9XG4gICAgcmV0dXJuIHJlbW92ZVNoaWZ0W29mZl0hO1xuICB9O1xuXG4gIC8vLyB2YXJpYWJsZXMgdXNlZCBieSBgaW50ZXJwcmV0T3JpZ2luVmlld2BcbiAgLy8vIHNjYW5uaW5nIHRoZSB0YXJnZXQgdmlld1xuICBsZXQgdGFyZ2V0Vmlld0N1cnNvciA9IDA7XG4gIC8vLyB0aGUgYXBwZW5kIGVmZmVjdFxuICBsZXQgYXBwZW5kT2Zmc2V0ID0gMDtcbiAgLy8vIGNvbnZlcnRlZCBhcHBlbmQgaW5zdHJ1Y3Rpb25zLlxuICBjb25zdCBzd2FwSW5zOiBudW1iZXJbXSA9IFtdO1xuICAvLy8gY29udmVydGVkIGFwcGVuZCBpbnN0cnVjdGlvbnMuXG4gIGNvbnN0IGluc2VydHM6IFsnaW5zZXJ0JywgbnVtYmVyLCBVXVtdID0gW107XG5cbiAgLy8vIGFwcGx5IGFwcGVuZCBhbmQgcmV1c2UgaW5zdHJ1Y3Rpb25zIHRpbGwgdGhlIG9mZnNldCBvZiBvcmlnaW4gc2VxdWVuY2UuXG4gIGNvbnN0IGludGVycHJldE9yaWdpblZpZXcgPSAob2ZmOiBudW1iZXIpID0+IHtcbiAgICAvLyBjb25zb2xlLmxvZyhvZmYsIGdldFNoaWZ0KG9mZikpO1xuICAgIG9mZiA9IGdldFNoaWZ0KG9mZik7XG4gICAgd2hpbGUgKHRhcmdldFZpZXdDdXJzb3IgPCB0YXJnZXRWaWV3Lmxlbmd0aCkge1xuICAgICAgbGV0IGRvbmUgPSBmYWxzZTtcbiAgICAgIGNvbnN0IGluc3QgPSB0YXJnZXRWaWV3W3RhcmdldFZpZXdDdXJzb3JdO1xuICAgICAgc3dpdGNoIChpbnN0WzBdKSB7XG4gICAgICAgIGNhc2UgJ2FwcGVuZCc6XG4gICAgICAgICAgaW5zZXJ0cy5wdXNoKFsnaW5zZXJ0JywgYXBwZW5kT2Zmc2V0LCBpbnN0WzFdXSk7XG4gICAgICAgICAgYXBwZW5kT2Zmc2V0Kys7XG4gICAgICAgICAgYnJlYWs7XG4gICAgICAgIGNhc2UgJ3JldXNlJzpcbiAgICAgICAgICBjb25zdCB0YXJnZXRfb2ZmID0gZ2V0U2hpZnQoaW5zdFsxXSk7XG4gICAgICAgICAgc3dhcElucy5wdXNoKHRhcmdldF9vZmYpO1xuICAgICAgICAgIGFwcGVuZE9mZnNldCsrO1xuICAgICAgICAgIGJyZWFrO1xuICAgICAgICAvLyBjYXNlIFwicmVtb3ZlXCI6XG4gICAgICAgIGRlZmF1bHQ6XG4gICAgICAgICAgYnJlYWs7XG4gICAgICB9XG5cbiAgICAgIHRhcmdldFZpZXdDdXJzb3IrKztcbiAgICAgIGlmIChkb25lKSB7XG4gICAgICAgIGJyZWFrO1xuICAgICAgfVxuICAgIH1cbiAgfTtcblxuICAvLy8gc2Nhbm5pbmcgdGhlIG9yaWdpbiB2aWV3XG4gIGZvciAobGV0IG9mZiA9IDA7IG9mZiA8IG9yaWdpbkNoaWxkcmVuLmxlbmd0aDsgb2ZmKyspIHtcbiAgICBjb25zdCBwcmV2Q2hpbGQgPSBvcmlnaW5DaGlsZHJlbltvZmZdO1xuXG4gICAgaWYgKHJlbW92ZVNoaWZ0W29mZl0gPT09IHVuZGVmaW5lZCkge1xuICAgICAgY29udGludWU7XG4gICAgfVxuXG4gICAgLy8ga2VlcCBwb3NpdGlvbiBvZiB1bnByZWRpY3RhYmxlIGVsZW1lbnRzXG4gICAgaWYgKCF0SXNVKHByZXZDaGlsZCkpIHtcbiAgICAgIGNvbnN0IHRhcmdldF9vZmYgPSBnZXRTaGlmdChvZmYpO1xuICAgICAgc3dhcElucy5wdXNoKHRhcmdldF9vZmYpO1xuICAgICAgY29udGludWU7XG4gICAgfVxuXG4gICAgaW50ZXJwcmV0T3JpZ2luVmlldyhvZmYpO1xuICB9XG4gIGludGVycHJldE9yaWdpblZpZXcob3JpZ2luQ2hpbGRyZW4ubGVuZ3RoKTtcblxuICBjb25zdCBzaW11bGF0ZWQ6IG51bWJlcltdID0gW107XG4gIGZvciAobGV0IGkgPSAwOyBpIDwgc3dhcElucy5sZW5ndGg7IGkrKykge1xuICAgIHNpbXVsYXRlZC5wdXNoKGkpO1xuICB9XG4gIGZvciAobGV0IGkgPSAwOyBpIDwgc3dhcElucy5sZW5ndGg7IGkrKykge1xuICAgIGNvbnN0IG9mZiA9IHN3YXBJbnNbaV07XG4gICAgZm9yIChsZXQgaiA9IDA7IGogPCBzaW11bGF0ZWQubGVuZ3RoOyBqKyspIHtcbiAgICAgIGlmIChzaW11bGF0ZWRbal0gPT09IG9mZikge1xuICAgICAgICAvLyBjb25zb2xlLmxvZyhcInN3YXBfaW5cIiwgaiwgaSwgc2ltdWxhdGVkKTtcbiAgICAgICAgc2ltdWxhdGVkLnNwbGljZShqLCAxKTtcbiAgICAgICAgaWYgKGkgPD0gaikge1xuICAgICAgICAgIHNpbXVsYXRlZC5zcGxpY2UoaSwgMCwgb2ZmKTtcbiAgICAgICAgfSBlbHNlIHtcbiAgICAgICAgICBzaW11bGF0ZWQuc3BsaWNlKGkgKyAxLCAwLCBvZmYpO1xuICAgICAgICB9XG4gICAgICAgIGlmIChqICE9PSBpKSB7XG4gICAgICAgICAgb3JpZ2luVmlldy5wdXNoKFsnc3dhcF9pbicsIGksIGpdKTtcbiAgICAgICAgICAvLyBjb25zb2xlLmxvZyhcInN3YXBfaW4gdGhlblwiLCBqLCBpLCBzaW11bGF0ZWQpO1xuICAgICAgICB9XG4gICAgICAgIGJyZWFrO1xuICAgICAgfVxuICAgIH1cbiAgfVxuXG4gIHJldHVybiBbLi4ub3JpZ2luVmlldywgLi4uaW5zZXJ0c107XG59XG5cbmZ1bmN0aW9uIHJ1bk9yaWdpblZpZXdJbnN0cnVjdGlvbnMocHJldjogRWxlbWVudCwgb3JpZ2luVmlldzogT3JpZ2luVmlld0luc3RydWN0aW9uPE5vZGU+W10pIHtcbiAgLy8gY29uc29sZS5sb2coXCJpbnRlcnByZXRlZCBvcmlnaW4gdmlld1wiLCBvcmlnaW5WaWV3KTtcbiAgZm9yIChjb25zdCBbb3AsIG9mZiwgZnJdIG9mIG9yaWdpblZpZXcpIHtcbiAgICBzd2l0Y2ggKG9wKSB7XG4gICAgICBjYXNlICdpbnNlcnQnOlxuICAgICAgICBwcmV2Lmluc2VydEJlZm9yZShmciwgcHJldi5jaGlsZHJlbltvZmZdKTtcbiAgICAgICAgYnJlYWs7XG4gICAgICBjYXNlICdzd2FwX2luJzpcbiAgICAgICAgcHJldi5pbnNlcnRCZWZvcmUocHJldi5jaGlsZHJlbltmcl0sIHByZXYuY2hpbGRyZW5bb2ZmXSk7XG4gICAgICAgIGJyZWFrO1xuICAgICAgY2FzZSAncmVtb3ZlJzpcbiAgICAgICAgcHJldi5jaGlsZHJlbltvZmZdLnJlbW92ZSgpO1xuICAgICAgICBicmVhaztcbiAgICAgIGRlZmF1bHQ6XG4gICAgICAgIHRocm93IG5ldyBFcnJvcigndW5rbm93biBvcCAnICsgb3ApO1xuICAgIH1cbiAgfVxufVxuXG4vLy8gRW5kIG9mIFZpZXcgSW50ZXJwcmV0YXRpb25cbi8vLyBCZWdpbiBvZiBSZWN1cnNpdmUgU3ZnIFBhdGNoXG5cbi8vLyBQYXRjaCB0aGUgYHByZXYgPHN2Zz5gIGluIHRoZSBET00gYWNjb3JkaW5nIHRvIGBuZXh0IDxzdmc+YCBmcm9tIHRoZSBiYWNrZW5kLlxuZXhwb3J0IGZ1bmN0aW9uIHBhdGNoUm9vdChwcmV2OiBTVkdFbGVtZW50LCBuZXh0OiBTVkdFbGVtZW50KSB7XG4gIC8vLyBQYXRjaCBhdHRyaWJ1dGVzXG4gIHBhdGNoQXR0cmlidXRlcyhwcmV2LCBuZXh0KTtcbiAgLy8vIFBhdGNoIGdsb2JhbCBzdmcgcmVzb3VyY2VzXG4gIHBhdGNoU3ZnSGVhZGVyKHByZXYsIG5leHQpO1xuXG4gIC8vLyBQYXRjaCBgPGc+YCBjaGlsZHJlbiwgY2FsbCBgcmV1c2VPclBhdGNoRWxlbWAgdG8gcGF0Y2guXG4gIHBhdGNoQ2hpbGRyZW4ocHJldiwgbmV4dCk7XG4gIHJldHVybjtcblxuICBmdW5jdGlvbiBwYXRjaFN2Z0hlYWRlcihwcmV2OiBTVkdFbGVtZW50LCBuZXh0OiBTVkdFbGVtZW50KSB7XG4gICAgZm9yIChsZXQgaSA9IDA7IGkgPCAzOyBpKyspIHtcbiAgICAgIGNvbnN0IHByZXZDaGlsZCA9IHByZXYuY2hpbGRyZW5baV07XG4gICAgICBjb25zdCBuZXh0Q2hpbGQgPSBuZXh0LmNoaWxkcmVuW2ldO1xuICAgICAgLy8gY29uc29sZS5sb2coXCJwcmV2XCIsIHByZXZDaGlsZCk7XG4gICAgICAvLyBjb25zb2xlLmxvZyhcIm5leHRcIiwgbmV4dENoaWxkKTtcbiAgICAgIGlmIChwcmV2Q2hpbGQudGFnTmFtZSA9PT0gJ2RlZnMnKSB7XG4gICAgICAgIGlmIChwcmV2Q2hpbGQuZ2V0QXR0cmlidXRlKCdjbGFzcycpID09PSAnZ2x5cGgnKSB7XG4gICAgICAgICAgLy8gY29uc29sZS5sb2coXCJhcHBlbmQgZ2x5cGhzOlwiLCBuZXh0Q2hpbGQuY2hpbGRyZW4sIFwidG9cIiwgcHJldkNoaWxkKTtcbiAgICAgICAgICBwcmV2Q2hpbGQuYXBwZW5kKC4uLm5leHRDaGlsZC5jaGlsZHJlbik7XG4gICAgICAgIH0gZWxzZSBpZiAocHJldkNoaWxkLmdldEF0dHJpYnV0ZSgnY2xhc3MnKSA9PT0gJ2NsaXAtcGF0aCcpIHtcbiAgICAgICAgICAvLyBjb25zb2xlLmxvZyhcImNsaXAgcGF0aDogcmVwbGFjZVwiKTtcbiAgICAgICAgICAvLyB0b2RvOiBnY1xuICAgICAgICAgIHByZXZDaGlsZC5hcHBlbmQoLi4ubmV4dENoaWxkLmNoaWxkcmVuKTtcbiAgICAgICAgfVxuICAgICAgfSBlbHNlIGlmIChwcmV2Q2hpbGQudGFnTmFtZSA9PT0gJ3N0eWxlJyAmJiBuZXh0Q2hpbGQuZ2V0QXR0cmlidXRlKCdkYXRhLXJldXNlJykgIT09ICcxJykge1xuICAgICAgICAvLyBjb25zb2xlLmxvZyhcInJlcGxhY2UgZXh0cmEgc3R5bGVcIiwgcHJldkNoaWxkLCBuZXh0Q2hpbGQpO1xuXG4gICAgICAgIC8vIHRvZG86IGdjXG4gICAgICAgIGlmIChuZXh0Q2hpbGQudGV4dENvbnRlbnQpIHtcbiAgICAgICAgICAvLyB0b2RvOiBsb29rcyBzbG93XG4gICAgICAgICAgLy8gaHR0cHM6Ly9zdGFja292ZXJmbG93LmNvbS9xdWVzdGlvbnMvMzMyNjQ5NC9wYXJzaW5nLWNzcy1pbi1qYXZhc2NyaXB0LWpxdWVyeVxuICAgICAgICAgIHZhciBkb2MgPSBkb2N1bWVudC5pbXBsZW1lbnRhdGlvbi5jcmVhdGVIVE1MRG9jdW1lbnQoJycpLFxuICAgICAgICAgICAgc3R5bGVFbGVtZW50ID0gZG9jdW1lbnQuY3JlYXRlRWxlbWVudCgnc3R5bGUnKTtcblxuICAgICAgICAgIHN0eWxlRWxlbWVudC50ZXh0Q29udGVudCA9IG5leHRDaGlsZC50ZXh0Q29udGVudDtcbiAgICAgICAgICAvLyB0aGUgc3R5bGUgd2lsbCBvbmx5IGJlIHBhcnNlZCBvbmNlIGl0IGlzIGFkZGVkIHRvIGEgZG9jdW1lbnRcbiAgICAgICAgICBkb2MuYm9keS5hcHBlbmRDaGlsZChzdHlsZUVsZW1lbnQpO1xuXG4gICAgICAgICAgY29uc3QgY3VycmVudFN2Z1NoZWV0ID0gKHByZXZDaGlsZCBhcyBIVE1MU3R5bGVFbGVtZW50KS5zaGVldCE7XG4gICAgICAgICAgY29uc3QgcnVsZXNUb0luc2VydCA9IHN0eWxlRWxlbWVudC5zaGVldD8uY3NzUnVsZXMgfHwgW107XG5cbiAgICAgICAgICAvLyBjb25zb2xlLmxvZyhcInJ1bGVzIHRvIGluc2VydFwiLCBjdXJyZW50U3ZnU2hlZXQsIHJ1bGVzVG9JbnNlcnQpO1xuICAgICAgICAgIGZvciAoY29uc3QgcnVsZSBvZiBydWxlc1RvSW5zZXJ0KSB7XG4gICAgICAgICAgICBjdXJyZW50U3ZnU2hlZXQuaW5zZXJ0UnVsZShydWxlLmNzc1RleHQpO1xuICAgICAgICAgIH1cbiAgICAgICAgfVxuICAgICAgfVxuICAgIH1cbiAgfVxufVxuXG4vLy8gYXBwbHkgYXR0cmlidXRlIHBhdGNoZXMgdG8gdGhlIGBwcmV2IDxzdmcgb3IgZz5gIGVsZW1lbnRcbmZ1bmN0aW9uIHBhdGNoQXR0cmlidXRlcyhwcmV2OiBFbGVtZW50LCBuZXh0OiBFbGVtZW50KSB7XG4gIGNvbnN0IHByZXZBdHRycyA9IHByZXYuYXR0cmlidXRlcztcbiAgY29uc3QgbmV4dEF0dHJzID0gbmV4dC5hdHRyaWJ1dGVzO1xuICBpZiAocHJldkF0dHJzLmxlbmd0aCA9PT0gbmV4dEF0dHJzLmxlbmd0aCkge1xuICAgIGxldCBzYW1lID0gdHJ1ZTtcbiAgICBmb3IgKGxldCBpID0gMDsgaSA8IHByZXZBdHRycy5sZW5ndGg7IGkrKykge1xuICAgICAgY29uc3QgcHJldkF0dHIgPSBwcmV2QXR0cnNbaV07XG4gICAgICBjb25zdCBuZXh0QXR0ciA9IG5leHRBdHRycy5nZXROYW1lZEl0ZW0ocHJldkF0dHIubmFtZSk7XG4gICAgICBpZiAobmV4dEF0dHIgPT09IG51bGwgfHwgcHJldkF0dHIudmFsdWUgIT09IG5leHRBdHRyLnZhbHVlKSB7XG4gICAgICAgIHNhbWUgPSBmYWxzZTtcbiAgICAgICAgYnJlYWs7XG4gICAgICB9XG4gICAgfVxuXG4gICAgaWYgKHNhbWUpIHtcbiAgICAgIC8vIGNvbnNvbGUubG9nKFwic2FtZSBhdHRyaWJ1dGVzLCBza2lwXCIpO1xuICAgICAgcmV0dXJuO1xuICAgIH1cbiAgfVxuICAvLyBjb25zb2xlLmxvZyhcImRpZmZlcmVudCBhdHRyaWJ1dGVzLCByZXBsYWNlXCIpO1xuXG4gIGNvbnN0IHJlbW92ZWRBdHRyczogc3RyaW5nW10gPSBbXTtcblxuICBmb3IgKGxldCBpID0gMDsgaSA8IHByZXZBdHRycy5sZW5ndGg7IGkrKykge1xuICAgIHJlbW92ZWRBdHRycy5wdXNoKHByZXZBdHRyc1tpXS5uYW1lKTtcbiAgfVxuXG4gIGZvciAoY29uc3QgYXR0ciBvZiByZW1vdmVkQXR0cnMpIHtcbiAgICBwcmV2LnJlbW92ZUF0dHJpYnV0ZShhdHRyKTtcbiAgfVxuXG4gIGZvciAobGV0IGkgPSAwOyBpIDwgbmV4dEF0dHJzLmxlbmd0aDsgaSsrKSB7XG4gICAgcHJldi5zZXRBdHRyaWJ1dGUobmV4dEF0dHJzW2ldLm5hbWUsIG5leHRBdHRyc1tpXS52YWx1ZSk7XG4gIH1cbn1cblxuLy8vIGFwcGx5IHBhdGNoZXMgdG8gdGhlIGNoaWxkcmVuIHNlcXVlbmNlIG9mIGBwcmV2IDxzdmcgb3IgZz5gIGluIHRoZSBET01cbmZ1bmN0aW9uIHBhdGNoQ2hpbGRyZW4ocHJldjogRWxlbWVudCwgbmV4dDogRWxlbWVudCkge1xuICBjb25zdCBbdGFyZ2V0VmlldywgdG9QYXRjaF0gPSBpbnRlcnByZXRUYXJnZXRWaWV3PFNWR0dFbGVtZW50PihcbiAgICBwcmV2LmNoaWxkcmVuIGFzIHVua25vd24gYXMgU1ZHR0VsZW1lbnRbXSxcbiAgICBuZXh0LmNoaWxkcmVuIGFzIHVua25vd24gYXMgU1ZHR0VsZW1lbnRbXSxcbiAgICBpc0dFbGVtLFxuICApO1xuXG4gIGZvciAobGV0IFtwcmV2Q2hpbGQsIG5leHRDaGlsZF0gb2YgdG9QYXRjaCkge1xuICAgIHJldXNlT3JQYXRjaEVsZW0ocHJldkNoaWxkLCBuZXh0Q2hpbGQpO1xuICB9XG5cbiAgLy8gY29uc29sZS5sb2coXCJpbnRlcnByZXRlZCB0YXJnZXQgdmlld1wiLCB0YXJnZXRWaWV3KTtcblxuICBjb25zdCBvcmlnaW5WaWV3ID0gY2hhbmdlVmlld1BlcnNwZWN0aXZlKFxuICAgIHByZXYuY2hpbGRyZW4gYXMgdW5rbm93biBhcyBTVkdHRWxlbWVudFtdLFxuICAgIHRhcmdldFZpZXcsXG4gICAgaXNHRWxlbSxcbiAgKTtcblxuICBydW5PcmlnaW5WaWV3SW5zdHJ1Y3Rpb25zKHByZXYsIG9yaWdpblZpZXcpO1xufVxuXG4vLy8gUmVwbGFjZSB0aGUgYHByZXZgIGVsZW1lbnQgd2l0aCBgbmV4dGAgZWxlbWVudC5cbi8vLyBSZXR1cm4gdHJ1ZSBpZiB0aGUgYHByZXZgIGVsZW1lbnQgaXMgcmV1c2VkLlxuLy8vIFJldHVybiBmYWxzZSBpZiB0aGUgYHByZXZgIGVsZW1lbnQgaXMgcmVwbGFjZWQuXG5mdW5jdGlvbiByZXVzZU9yUGF0Y2hFbGVtKHByZXY6IFNWR0dFbGVtZW50LCBuZXh0OiBTVkdHRWxlbWVudCkge1xuICBjb25zdCBjYW5SZXVzZSA9IGVxdWFsRWxlbShwcmV2LCBuZXh0KTtcblxuICAvLy8gRXZlbiBpZiB0aGUgZWxlbWVudCBpcyByZXVzZWQsIHdlIHN0aWxsIG5lZWQgdG8gcmVwbGFjZSBpdHMgYXR0cmlidXRlcy5cbiAgbmV4dC5yZW1vdmVBdHRyaWJ1dGUoVHlwc3RTdmdBdHRycy5SZXVzZUZyb20pO1xuICBwYXRjaEF0dHJpYnV0ZXMocHJldiwgbmV4dCk7XG5cbiAgaWYgKGNhblJldXNlKSB7XG4gICAgcmV0dXJuIHRydWUgLyogcmV1c2VkICovO1xuICB9XG5cbiAgLy8vIEhhcmQgcmVwbGFjZSBlbGVtZW50cyB0aGF0IGlzIG5vdCBhIGA8Zz5gIGVsZW1lbnQuXG4gIHJlcGxhY2VOb25TVkdFbGVtZW50cyhwcmV2LCBuZXh0KTtcbiAgLy8vIFBhdGNoIGA8Zz5gIGNoaWxkcmVuLCB3aWxsIGNhbGwgYHJldXNlT3JQYXRjaEVsZW1gIGFnYWluLlxuICBwYXRjaENoaWxkcmVuKHByZXYsIG5leHQpO1xuICByZXR1cm4gZmFsc2UgLyogcmV1c2VkICovO1xuXG4gIGZ1bmN0aW9uIHJlcGxhY2VOb25TVkdFbGVtZW50cyhwcmV2OiBFbGVtZW50LCBuZXh0OiBFbGVtZW50KSB7XG4gICAgY29uc3QgcmVtb3ZlZEluZGljZXM6IG51bWJlcltdID0gW107XG4gICAgZm9yIChsZXQgaSA9IDA7IGkgPCBwcmV2LmNoaWxkcmVuLmxlbmd0aDsgaSsrKSB7XG4gICAgICBjb25zdCBwcmV2Q2hpbGQgPSBwcmV2LmNoaWxkcmVuW2ldO1xuICAgICAgaWYgKCFpc0dFbGVtKHByZXZDaGlsZCkpIHtcbiAgICAgICAgcmVtb3ZlZEluZGljZXMucHVzaChpKTtcbiAgICAgIH1cbiAgICB9XG5cbiAgICBmb3IgKGNvbnN0IGluZGV4IG9mIHJlbW92ZWRJbmRpY2VzLnJldmVyc2UoKSkge1xuICAgICAgcHJldi5jaGlsZHJlbltpbmRleF0ucmVtb3ZlKCk7XG4gICAgfVxuXG4gICAgZm9yIChsZXQgaSA9IDA7IGkgPCBuZXh0LmNoaWxkcmVuLmxlbmd0aDsgaSsrKSB7XG4gICAgICBjb25zdCBuZXh0Q2hpbGQgPSBuZXh0LmNoaWxkcmVuW2ldO1xuICAgICAgaWYgKCFpc0dFbGVtKG5leHRDaGlsZCkpIHtcbiAgICAgICAgcHJldi5hcHBlbmRDaGlsZChuZXh0Q2hpbGQuY2xvbmVOb2RlKHRydWUpKTtcbiAgICAgIH1cbiAgICB9XG4gIH1cbn1cblxuLy8vIEVuZCBvZiBSZWN1cnNpdmUgU3ZnIFBhdGNoXG4iXX0=