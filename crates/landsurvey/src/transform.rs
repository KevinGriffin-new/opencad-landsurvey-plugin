//! 2-D conformal (similarity) coordinate transform — Rotate / Translate / Scale.
//!
//! Pure functions, `std` only. Ported from the reference `geodesy/transform.py`.
//! Coordinates are `(easting, northing)`. The 4-parameter conformal transform is
//!
//! ```text
//!     E' = a*E - b*N + c
//!     N' = b*E + a*N + d           with  a = s*cos(theta), b = s*sin(theta)
//! ```
//!
//! where `s` is scale and `theta` is rotation (math convention: CCW positive in
//! the E-N plane). Two ways to build one:
//! * [`Conformal::from_base_swing`] — an explicit Rotate/Translate/Scale pinned
//!   to a base point (the survey "rotate-translate-scale about a monument").
//! * [`helmert_fit`] — least-squares best fit from control-point pairs (exact
//!   for two pairs).

/// A 4-parameter 2-D conformal transform over `(easting, northing)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conformal {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
}

impl Conformal {
    /// The identity transform.
    pub fn identity() -> Self {
        Conformal { a: 1.0, b: 0.0, c: 0.0, d: 0.0 }
    }

    /// Scale factor `s = hypot(a, b)`.
    pub fn scale(&self) -> f64 {
        self.a.hypot(self.b)
    }

    /// Rotation in degrees (math convention, CCW positive).
    pub fn rotation_deg(&self) -> f64 {
        self.b.atan2(self.a).to_degrees()
    }

    /// Apply to a source `(E, N)` -> destination `(E', N')`.
    pub fn apply(&self, e: f64, n: f64) -> (f64, f64) {
        (self.a * e - self.b * n + self.c, self.b * e + self.a * n + self.d)
    }

    /// Decompose applying this transform to `(e, n)` into four geometric stages
    /// about an arbitrary pivot `G`:
    /// `[source, scaled-about-G, +rotated-about-G, +translated]`. Stage 3 always
    /// equals `apply(e, n)`, so the decomposition is exact regardless of `G`
    /// (it only changes which point stays fixed during scale/rotate).
    pub fn stages_about(&self, e: f64, n: f64, pivot: (f64, f64)) -> [(f64, f64); 4] {
        let s = self.scale();
        let th = self.rotation_deg().to_radians();
        let (gx, gy) = pivot;
        let (dx, dy) = (e - gx, n - gy);
        let p1 = (gx + s * dx, gy + s * dy);
        let (sx, sy) = (s * dx, s * dy);
        let p2 = (gx + th.cos() * sx - th.sin() * sy, gy + th.sin() * sx + th.cos() * sy);
        [(e, n), p1, p2, self.apply(e, n)]
    }

    /// Build a Rotate/Translate/Scale transform that pins `base_src` -> `base_dst`,
    /// rotates by `swing_deg` (CCW+), and scales by `scale` about the base.
    /// `base_dst == base_src` means rotate/scale in place (no translation);
    /// `swing_deg = 0`, `scale = 1` means a pure translation `base_src -> base_dst`.
    pub fn from_base_swing(
        base_src: (f64, f64),
        base_dst: (f64, f64),
        swing_deg: f64,
        scale: f64,
    ) -> Self {
        let r = swing_deg.to_radians();
        let a = scale * r.cos();
        let b = scale * r.sin();
        let (es, ns) = base_src;
        let (ed, nd) = base_dst;
        let c = ed - (a * es - b * ns);
        let d = nd - (b * es + a * ns);
        Conformal { a, b, c, d }
    }
}

/// Every intermediate value of a least-squares 2-D Helmert fit, so the steps
/// can be reported and drawn. Coordinates are `(E, N)`.
#[derive(Debug, Clone, Copy)]
pub struct HelmertSteps {
    /// Number of control pairs used.
    pub n: usize,
    /// Step 1 — centroid of the source points.
    pub src_centroid: (f64, f64),
    /// Step 2 — centroid of the destination points.
    pub dst_centroid: (f64, f64),
    /// Step 3 — cross-covariance sums of the centred points:
    /// `sxx = Σ(dxs·dxt + dys·dyt)`, `sxy = Σ(dxs·dyt − dys·dxt)`,
    /// `sum_sq = Σ(dxs² + dys²)`.
    pub sxx: f64,
    pub sxy: f64,
    pub sum_sq: f64,
    /// The resulting 4 parameters (`a = s·cosθ`, `b = s·sinθ`, plus translation).
    pub transform: Conformal,
}

impl HelmertSteps {
    pub fn scale(&self) -> f64 {
        self.transform.scale()
    }
    pub fn rotation_deg(&self) -> f64 {
        self.transform.rotation_deg()
    }

    /// The four geometric application stages for a source point — scale and
    /// rotate about the **source centroid**, then translate to the target
    /// centroid. Stage 3 equals the full fitted transform.
    pub fn stages(&self, e: f64, n: f64) -> [(f64, f64); 4] {
        self.transform.stages_about(e, n, self.src_centroid)
    }
}

/// Least-squares 4-parameter conformal fit with all intermediate values
/// exposed (see [`HelmertSteps`]). Needs >= 2 pairs.
pub fn helmert_fit_explained(
    pairs: &[((f64, f64), (f64, f64))],
) -> Result<HelmertSteps, &'static str> {
    let n = pairs.len();
    if n < 2 {
        return Err("helmert_fit needs at least 2 control pairs");
    }
    // f64::parse accepts "NaN"/"inf"; a non-finite pair sails through the
    // degenerate-geometry guard below (NaN comparisons are false) and yields a
    // transform of NaNs that downstream code would render or apply.
    if pairs
        .iter()
        .any(|&((a, b), (c, d))| ![a, b, c, d].iter().all(|v| v.is_finite()))
    {
        return Err("control pairs contain non-finite coordinates");
    }
    let nf = n as f64;
    let (mut cx, mut cy, mut cx2, mut cy2) = (0.0, 0.0, 0.0, 0.0);
    for &((es, ns), (ed, nd)) in pairs {
        cx += es;
        cy += ns;
        cx2 += ed;
        cy2 += nd;
    }
    cx /= nf;
    cy /= nf;
    cx2 /= nf;
    cy2 /= nf;

    let (mut sxx, mut sxy, mut den) = (0.0, 0.0, 0.0);
    for &((es, ns), (ed, nd)) in pairs {
        let (dxs, dys) = (es - cx, ns - cy);
        let (dxt, dyt) = (ed - cx2, nd - cy2);
        sxx += dxs * dxt + dys * dyt;
        sxy += dxs * dyt - dys * dxt;
        den += dxs * dxs + dys * dys;
    }
    if den.abs() < 1e-12 {
        return Err("degenerate control geometry (coincident source points)");
    }
    let a = sxx / den;
    let b = sxy / den;
    let c = cx2 - (a * cx - b * cy);
    let d = cy2 - (b * cx + a * cy);
    Ok(HelmertSteps {
        n,
        src_centroid: (cx, cy),
        dst_centroid: (cx2, cy2),
        sxx,
        sxy,
        sum_sq: den,
        transform: Conformal { a, b, c, d },
    })
}

/// Least-squares 4-parameter conformal fit from control-point pairs
/// `((e_src, n_src), (e_dst, n_dst))`. Needs >= 2 pairs (exact for 2). Returns
/// `Err` for fewer pairs or a degenerate (coincident-source) configuration.
pub fn helmert_fit(pairs: &[((f64, f64), (f64, f64))]) -> Result<Conformal, &'static str> {
    helmert_fit_explained(pairs).map(|s| s.transform)
}

/// Per-pair residual distances and their RMS for a fitted transform.
pub fn fit_residuals(t: &Conformal, pairs: &[((f64, f64), (f64, f64))]) -> (Vec<f64>, f64) {
    let mut res = Vec::with_capacity(pairs.len());
    for &((es, ns), (ed, nd)) in pairs {
        let (ep, np_) = t.apply(es, ns);
        res.push((ep - ed).hypot(np_ - nd));
    }
    let rms = if res.is_empty() {
        0.0
    } else {
        (res.iter().map(|r| r * r).sum::<f64>() / res.len() as f64).sqrt()
    };
    (res, rms)
}

/// Parse control-point pairs for a Helmert fit. Each non-blank, non-`#` line is
/// `srcN, srcE, dstN, dstE` (comma / whitespace separated), with an optional
/// point ID as the first or last field, recognized only when it is
/// non-numeric (`CP1`); an all-numeric 5-field line is rejected as ambiguous
/// rather than guessed at. Returns pairs as
/// `((srcE, srcN), (dstE, dstN))` — the `(E, N)` order [`helmert_fit`] expects.
///
/// Malformed lines are an error, not a skip: a stray token or an extra column
/// would otherwise shift the remaining coordinates one slot left and produce a
/// garbage fit that still reports a plausible RMS.
pub fn parse_control_pairs(text: &str) -> Result<Vec<((f64, f64), (f64, f64))>, String> {
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let mut toks: Vec<&str> = l
            .split([',', ' ', '\t'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let numeric = |t: &str| t.parse::<f64>().is_ok();
        // 5 fields = a point ID at one end. A non-numeric end identifies
        // itself; all-numeric is ambiguous (leading point *number* vs trailing
        // ID) and silently guessing wrong would shift every coordinate.
        if toks.len() == 5 {
            match (numeric(toks[0]), numeric(toks[4])) {
                (false, true) => {
                    toks.remove(0);
                }
                (true, false) => {
                    toks.pop();
                }
                _ => {
                    return Err(format!(
                        "control line {}: 5 numeric fields is ambiguous (point \
                         ID first or last?) — use 4 fields `srcN srcE dstN dstE` \
                         or a non-numeric ID: \"{l}\"",
                        idx + 1
                    ));
                }
            }
        }
        if toks.len() != 4 {
            return Err(format!(
                "control line {}: expected `srcN srcE dstN dstE` (with an \
                 optional ID first or last), got {} field(s): \"{l}\"",
                idx + 1,
                toks.len()
            ));
        }
        let mut nums = [0.0f64; 4];
        for (n, t) in nums.iter_mut().zip(&toks) {
            *n = match t.parse::<f64>() {
                Ok(v) if v.is_finite() => v,
                _ => {
                    return Err(format!(
                        "control line {}: \"{t}\" is not a finite number: \"{l}\"",
                        idx + 1
                    ))
                }
            };
        }
        out.push(((nums[1], nums[0]), (nums[3], nums[2])));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn helmert_steps_stage3_equals_full_transform() {
        let truth = Conformal::from_base_swing((10.0, 20.0), (300.0, 400.0), 25.0, 1.3);
        let src = [(0.0, 0.0), (40.0, 5.0), (15.0, 60.0), (70.0, 30.0), (22.0, 18.0)];
        let pairs: Vec<_> = src.iter().map(|&(e, n)| ((e, n), truth.apply(e, n))).collect();
        let steps = helmert_fit_explained(&pairs).unwrap();
        // Centroid maps to centroid.
        let (gx, gy) = steps.src_centroid;
        let (mx, my) = steps.transform.apply(gx, gy);
        assert!(close(mx, steps.dst_centroid.0) && close(my, steps.dst_centroid.1));
        // Every point's stage 3 equals applying the whole transform.
        for &(e, n) in &src {
            let st = steps.stages(e, n);
            let (fe, fnn) = steps.transform.apply(e, n);
            assert!(close(st[3].0, fe) && close(st[3].1, fnn));
        }
        assert!(close(steps.scale(), 1.3));
    }

    #[test]
    fn parse_pairs_accepts_ids_and_orders_en() {
        let text = "# srcN, srcE, dstN, dstE\n\
                    1000, 2000, 5000, 6000, CP1\n\
                    \n\
                    CP2, 1100, 2100, 5100, 6100\n\
                    1200 2200 5200 6200\n";
        let pairs = parse_control_pairs(text).expect("valid control file");
        assert_eq!(pairs.len(), 3);
        // (srcE, srcN) then (dstE, dstN)
        assert_eq!(pairs[0], ((2000.0, 1000.0), (6000.0, 5000.0)));
        assert_eq!(pairs[1], ((2100.0, 1100.0), (6100.0, 5100.0)));
        assert_eq!(pairs[2], ((2200.0, 1200.0), (6200.0, 5200.0)));
    }

    #[test]
    fn parse_pairs_rejects_malformed_lines_loudly() {
        // Junk text: previously silently skipped.
        assert!(parse_control_pairs("bad line\n").is_err());
        // All-numeric 5 fields: previously consumed the point number as srcN
        // and silently discarded dstE — the column-shift trap.
        assert!(parse_control_pairs("101, 1000, 2000, 5000, 6000\n").is_err());
        // Too many / too few fields.
        assert!(parse_control_pairs("1 2 3 4 5 6\n").is_err());
        assert!(parse_control_pairs("1 2 3\n").is_err());
        // Non-finite coordinates: f64::parse accepts these strings.
        assert!(parse_control_pairs("NaN 2000 5000 6000\n").is_err());
        assert!(parse_control_pairs("1000 inf 5000 6000\n").is_err());
    }

    #[test]
    fn helmert_fit_rejects_non_finite_pairs() {
        let pairs = [((0.0, 0.0), (10.0, 10.0)), ((f64::NAN, 5.0), (12.0, 15.0))];
        assert!(helmert_fit(&pairs).is_err());
    }

    #[test]
    fn pure_translation() {
        let t = Conformal::from_base_swing((0.0, 0.0), (10.0, 5.0), 0.0, 1.0);
        let (e, n) = t.apply(3.0, 4.0);
        assert!(close(e, 13.0) && close(n, 9.0));
        assert!(close(t.scale(), 1.0));
        assert!(close(t.rotation_deg(), 0.0));
    }

    #[test]
    fn rotate_90_about_origin() {
        // CCW 90 about origin: (1,0) -> (0,1).
        let t = Conformal::from_base_swing((0.0, 0.0), (0.0, 0.0), 90.0, 1.0);
        let (e, n) = t.apply(1.0, 0.0);
        assert!(close(e, 0.0) && close(n, 1.0), "got ({e},{n})");
        assert!(close(t.rotation_deg(), 90.0));
    }

    #[test]
    fn scale_2_about_base() {
        let t = Conformal::from_base_swing((1.0, 1.0), (1.0, 1.0), 0.0, 2.0);
        // base is fixed; a point 1 east of base moves to 2 east of base.
        let (e, n) = t.apply(2.0, 1.0);
        assert!(close(e, 3.0) && close(n, 1.0), "got ({e},{n})");
        assert!(close(t.scale(), 2.0));
    }

    #[test]
    fn helmert_recovers_known_transform() {
        let truth = Conformal::from_base_swing((100.0, 200.0), (500.0, 600.0), 30.0, 1.5);
        let src = [(0.0, 0.0), (50.0, 10.0), (20.0, 80.0), (90.0, 40.0)];
        let pairs: Vec<_> = src.iter().map(|&(e, n)| ((e, n), truth.apply(e, n))).collect();
        let fit = helmert_fit(&pairs).unwrap();
        assert!(close(fit.a, truth.a) && close(fit.b, truth.b));
        assert!(close(fit.c, truth.c) && close(fit.d, truth.d));
        let (_, rms) = fit_residuals(&fit, &pairs);
        assert!(rms < 1e-6, "rms {rms}");
        assert!(close(fit.scale(), 1.5));
        assert!(close(fit.rotation_deg(), 30.0));
    }

    #[test]
    fn fit_reports_residuals_on_noisy_data() {
        // Exact except one perturbed target -> non-zero rms.
        let _truth = Conformal::from_base_swing((0.0, 0.0), (0.0, 0.0), 0.0, 1.0);
        let mut pairs: Vec<_> =
            [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)].iter().map(|&p| (p, p)).collect();
        pairs.push(((10.0, 10.0), (10.1, 10.0))); // 0.1 ft off
        let fit = helmert_fit(&pairs).unwrap();
        let (res, rms) = fit_residuals(&fit, &pairs);
        assert_eq!(res.len(), 4);
        assert!(rms > 0.0 && rms < 0.1);
    }
}
