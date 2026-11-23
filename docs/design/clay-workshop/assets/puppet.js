// Desk coordinates use 512 px puppet cells. Each pose is one finished sculpt.
function makeClayPuppet(film, selectors) {
  const move = (target, from, to, at, duration, ease) =>
    film.fromTo(target, from, { ...to, duration, ease, immediateRender: false }, at);

  // Sculpts 0-5 are the model sheet. An episode's own sculpts follow from 6,
  // laid out in one row to the right of the sheet.
  function pose(index, at) {
    const x = index < 6 ? index % 3 * 512 : 1536 + (index - 6) * 512;
    const y = index < 6 ? Math.floor(index / 3) * 512 : 0;
    film.set(selectors.sculpt, { attr: { viewBox: x + " " + y + " 512 512" } }, at);
  }

  // Lid rows follow the sculpts that have them: 1 is the push, 2 is the thumbs-up.
  function lids(lidRow, column, at) {
    film.set(selectors.lids, {
      opacity: 1,
      backgroundPosition: -column * 512 + "px " + -lidRow * 512 + "px"
    }, at);
  }

  const openEyes = at => film.set(selectors.lids, { opacity: 0 }, at);

  function blink(lidRow, at) {
    lids(lidRow, 0, at);
    lids(lidRow, 1, at + .066);
    lids(lidRow, 0, at + .133);
    openEyes(at + .20);
  }

  // The sculpt changes at the bottom of a squash, so the eye reads one body
  // changing shape. Returns the time the new sculpt appears.
  function swap(index, at) {
    const flip = at + .10;
    openEyes(at);
    move(selectors.pose, { scaleX: 1, scaleY: 1 }, { scaleX: 1.08, scaleY: .88 }, at, .10, "power2.in");
    pose(index, flip);
    move(selectors.pose, { scaleX: 1.08, scaleY: .88 }, { scaleX: .96, scaleY: 1.06 }, flip, .09, "power2.out");
    move(selectors.pose, { scaleX: .96, scaleY: 1.06 }, { scaleX: 1, scaleY: 1 }, flip + .09, .22, "back.out(2)");
    return flip;
  }

  const HIPS = 4, PUSH = 3, THUMB = 5;
  const whole = [selectors.body, selectors.shadow];

  // Miga waits below the frame. Call before the timeline starts.
  function wait(home) {
    gsap.set(whole, { x: home });
    gsap.set(selectors.body, { y: 500 });
    pose(HIPS, 0);
  }

  function arrive(at) {
    move(selectors.body, { y: 500 }, { y: 0 }, at, .42, "back.out(1.5)");
    move(selectors.shadow, { opacity: 0 }, { opacity: 1 }, at + .12, .25, "power1.out");
  }

  // Wind-up and shove from her home position. `hit` is when the prop takes
  // the push. `letGo` is when she is back on her feet.
  function shove(home, at) {
    swap(PUSH, at);
    move(selectors.pose, { rotation: 0 }, { rotation: -6 }, at + .20, .28, "power2.out");
    lids(1, 0, at + .36);
    lids(1, 1, at + .42);
    move(selectors.pose, { rotation: -6 }, { rotation: 4 }, at + .52, .12, "power3.in");
    move(whole, { x: home }, { x: home + 26 }, at + .54, .14, "power3.in");
    const hit = at + .62;
    const letGo = swap(HIPS, hit + .22);
    film.set(whole, { x: home }, letGo);
    film.set(selectors.pose, { rotation: 0 }, letGo);
    return { hit, letGo };
  }

  const thumbsUp = at => swap(THUMB, at);

  // Whoever sat here has just left: the chair is still turning.
  function emptyChair(chair) {
    move(chair, { scaleX: 1 }, { scaleX: -1 }, 0, .22, "sine.inOut");
    move(chair, { scaleX: -1 }, { scaleX: 1 }, .22, .34, "sine.inOut");
    move(chair, { rotation: -3 }, { rotation: 0 }, .56, .70, "elastic.out(1,.4)");
  }

  function dust(specks, at) {
    move(specks, { scale: .3, opacity: .9, x: 0, y: 0 },
      { scale: 1.5, opacity: 0, x: i => [-46, -8, 34][i], y: i => [-18, -40, -14][i] }, at, .45, "power2.out");
  }

  return { move, pose, swap, blink, wait, arrive, shove, thumbsUp, emptyChair, dust };
}
