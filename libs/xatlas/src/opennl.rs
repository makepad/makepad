//! OpenNL conjugate-gradient solver from `vendor/xatlas.cpp:3928`.

const NL_MATRIX_SPARSE_DYNAMIC: u32 = 0x1001;
const NL_MATRIX_CRS: u32 = 0x1002;
const NL_MATRIX_OTHER: u32 = 0x1006;

pub const NL_NB_VARIABLES: u32 = 0x101;
pub const NL_MAX_ITERATIONS: u32 = 0x103;
pub const NL_SYSTEM: u32 = 0x0;
pub const NL_MATRIX: u32 = 0x1;
pub const NL_ROW: u32 = 0x2;

#[derive(Clone, Copy, Default)]
struct NlCoeff {
    index: u32,
    value: f64,
}

#[derive(Default)]
struct NlRowColumn {
    coeff: Vec<NlCoeff>,
}

impl NlRowColumn {
    fn add(&mut self, index: u32, value: f64) {
        for c in &mut self.coeff {
            if c.index == index {
                c.value += value;
                return;
            }
        }
        self.coeff.push(NlCoeff { index, value });
    }

    fn append(&mut self, index: u32, value: f64) {
        self.coeff.push(NlCoeff { index, value });
    }

    fn zero(&mut self) {
        self.coeff.clear();
    }

    fn sort(&mut self) {
        // Unique indices; sort ascending to match the intended CRS order.
        // xatlas.cpp:4118 uses a broken qsort comparator; unique keys make
        // a proper ascending sort the only well-defined result.
        self.coeff.sort_by_key(|c| c.index);
    }
}

struct SparseMatrix {
    m: u32,
    n: u32,
    row: Vec<NlRowColumn>,
    diag: Vec<f64>,
}

impl SparseMatrix {
    fn new(m: u32, n: u32) -> Self {
        let diag_size = m.min(n);
        Self {
            m,
            n,
            row: (0..m).map(|_| NlRowColumn::default()).collect(),
            diag: vec![0.0; diag_size as usize],
        }
    }

    fn add(&mut self, i: u32, j: u32, value: f64) {
        if i == j {
            self.diag[i as usize] += value;
        }
        self.row[i as usize].add(j, value);
    }

    fn sort(&mut self) {
        for r in &mut self.row {
            r.sort();
        }
    }
}

struct CrsMatrix {
    m: u32,
    n: u32,
    val: Vec<f64>,
    rowptr: Vec<u32>,
    colind: Vec<u32>,
    sliceptr: Vec<u32>,
}

impl CrsMatrix {
    fn from_sparse(m: &SparseMatrix) -> Self {
        let nnz: u32 = m.row.iter().map(|r| r.coeff.len() as u32).sum();
        let nslices = 8u32;
        let slice_size = nnz / nslices;
        let mut crs = Self {
            m: m.m,
            n: m.n,
            val: vec![0.0; nnz as usize],
            rowptr: vec![0; m.m as usize + 1],
            colind: vec![0; nnz as usize],
            sliceptr: vec![0; nslices as usize + 1],
        };
        let mut sm = SparseMatrix {
            m: m.m,
            n: m.n,
            row: m.row.iter().map(|r| NlRowColumn { coeff: r.coeff.clone() }).collect(),
            diag: m.diag.clone(),
        };
        sm.sort();
        let mut k = 0u32;
        for i in 0..sm.m {
            let ri = &sm.row[i as usize];
            crs.rowptr[i as usize] = k;
            for c in &ri.coeff {
                crs.val[k as usize] = c.value;
                crs.colind[k as usize] = c.index;
                k += 1;
            }
        }
        crs.rowptr[sm.m as usize] = k;
        let mut cur_bound = slice_size;
        let mut cur_nnz = 0u32;
        let mut cur_row = 0u32;
        crs.sliceptr[0] = 0;
        for slice in 1..nslices {
            while cur_nnz < cur_bound && cur_row < sm.m {
                cur_nnz += crs.rowptr[cur_row as usize + 1] - crs.rowptr[cur_row as usize];
                cur_row += 1;
            }
            crs.sliceptr[slice as usize] = cur_row;
            cur_bound += slice_size;
        }
        crs.sliceptr[nslices as usize] = sm.m;
        crs
    }

    fn mult(&self, x: &[f64], y: &mut [f64]) {
        for i in 0..self.m as usize {
            let mut sum = 0.0;
            for j in self.rowptr[i]..self.rowptr[i + 1] {
                sum += self.val[j as usize] * x[self.colind[j as usize] as usize];
            }
            y[i] = sum;
        }
    }
}

enum Matrix {
    Sparse(SparseMatrix),
    Crs(CrsMatrix),
    Jacobi { n: u32, diag_inv: Vec<f64> },
}

impl Matrix {
    fn n(&self) -> u32 {
        match self {
            Matrix::Sparse(m) => m.n,
            Matrix::Crs(m) => m.n,
            Matrix::Jacobi { n, .. } => *n,
        }
    }

    fn m(&self) -> u32 {
        match self {
            Matrix::Sparse(m) => m.m,
            Matrix::Crs(m) => m.m,
            Matrix::Jacobi { n, .. } => *n,
        }
    }

    fn ty(&self) -> u32 {
        match self {
            Matrix::Sparse(_) => NL_MATRIX_SPARSE_DYNAMIC,
            Matrix::Crs(_) => NL_MATRIX_CRS,
            Matrix::Jacobi { .. } => NL_MATRIX_OTHER,
        }
    }

    fn mult(&self, x: &[f64], y: &mut [f64]) {
        match self {
            Matrix::Sparse(m) => {
                for i in 0..m.m as usize {
                    y[i] = 0.0;
                    for c in &m.row[i].coeff {
                        y[i] += c.value * x[c.index as usize];
                    }
                }
            }
            Matrix::Crs(m) => m.mult(x, y),
            Matrix::Jacobi { diag_inv, .. } => {
                for i in 0..diag_inv.len() {
                    y[i] = x[i] * diag_inv[i];
                }
            }
        }
    }
}

fn jacobi_from_sparse(m: &SparseMatrix) -> Matrix {
    let mut diag_inv = vec![0.0; m.n as usize];
    for i in 0..m.n as usize {
        diag_inv[i] = if m.diag[i] == 0.0 { 1.0 } else { 1.0 / m.diag[i] };
    }
    Matrix::Jacobi {
        n: m.n,
        diag_inv,
    }
}

pub struct NlContext {
    variable_value: Vec<f64>,
    variable_is_locked: Vec<bool>,
    variable_index: Vec<u32>,
    n: u32,
    m: Option<Matrix>,
    p: Option<Matrix>,
    af: NlRowColumn,
    al: NlRowColumn,
    x: Vec<f64>,
    b: Vec<f64>,
    nb_variables: u32,
    nb_systems: u32,
    current_row: u32,
    max_iterations: u32,
    max_iterations_defined: bool,
    threshold: f64,
}

impl NlContext {
    pub fn new() -> Self {
        Self {
            variable_value: Vec::new(),
            variable_is_locked: Vec::new(),
            variable_index: Vec::new(),
            n: 0,
            m: None,
            p: None,
            af: NlRowColumn::default(),
            al: NlRowColumn::default(),
            x: Vec::new(),
            b: Vec::new(),
            nb_variables: 0,
            nb_systems: 1,
            current_row: 0,
            max_iterations: 100,
            max_iterations_defined: false,
            threshold: 1e-6,
        }
    }
}

fn ddot(n: usize, x: &[f64], y: &[f64]) -> f64 {
    let mut sum = 0.0;
    for i in 0..n {
        sum += x[i] * y[i];
    }
    sum
}

fn daxpy(n: usize, a: f64, x: &[f64], y: &mut [f64]) {
    for i in 0..n {
        y[i] = a * x[i] + y[i];
    }
}

fn dscal(n: usize, a: f64, x: &mut [f64]) {
    for i in 0..n {
        x[i] *= a;
    }
}

fn solve_pre_cg(m: &Matrix, p: &Matrix, b: &[f64], x: &mut [f64], eps: f64, max_iter: u32) -> u32 {
    let n = m.n() as usize;
    let mut r = vec![0.0; n];
    let mut d = vec![0.0; n];
    let mut h = vec![0.0; n];
    let mut its = 0u32;
    let b_square = ddot(n, b, b);
    let err = eps * eps * b_square;
    m.mult(x, &mut r);
    daxpy(n, -1.0, b, &mut r);
    p.mult(&r, &mut d);
    h.copy_from_slice(&d);
    let mut rh = ddot(n, &r, &h);
    let mut curr_err = ddot(n, &r, &r);
    while curr_err > err && its < max_iter {
        let mut ad = vec![0.0; n];
        m.mult(&d, &mut ad);
        let alpha = rh / ddot(n, &d, &ad);
        daxpy(n, -alpha, &d, x);
        daxpy(n, -alpha, &ad, &mut r);
        p.mult(&r, &mut h);
        let mut beta = 1.0 / rh;
        rh = ddot(n, &r, &h);
        beta *= rh;
        dscal(n, beta, &mut d);
        daxpy(n, 1.0, &h, &mut d);
        its += 1;
        curr_err = ddot(n, &r, &r);
    }
    its
}

pub fn nl_solver_parameteri(ctx: &mut NlContext, pname: u32, param: i32) {
    if pname == NL_NB_VARIABLES {
        ctx.nb_variables = param as u32;
    } else if pname == NL_MAX_ITERATIONS {
        ctx.max_iterations = param as u32;
        ctx.max_iterations_defined = true;
    }
}

pub fn nl_set_variable(ctx: &mut NlContext, index: u32, value: f64) {
    ctx.variable_value[index as usize] = value;
}

pub fn nl_get_variable(ctx: &NlContext, index: u32) -> f64 {
    ctx.variable_value[index as usize]
}

pub fn nl_lock_variable(ctx: &mut NlContext, index: u32) {
    ctx.variable_is_locked[index as usize] = true;
}

fn nl_variables_to_vector(ctx: &mut NlContext) {
    let n = ctx.n;
    for k in 0..ctx.nb_systems {
        for i in 0..ctx.nb_variables {
            if !ctx.variable_is_locked[i as usize] {
                let index = ctx.variable_index[i as usize];
                let value = ctx.variable_value[(k * ctx.nb_variables + i) as usize];
                ctx.x[(index + k * n) as usize] = value;
            }
        }
    }
}

fn nl_vector_to_variables(ctx: &mut NlContext) {
    let n = ctx.n;
    for k in 0..ctx.nb_systems {
        for i in 0..ctx.nb_variables {
            if !ctx.variable_is_locked[i as usize] {
                let index = ctx.variable_index[i as usize];
                let value = ctx.x[(index + k * n) as usize];
                ctx.variable_value[(k * ctx.nb_variables + i) as usize] = value;
            }
        }
    }
}

pub fn nl_coefficient(ctx: &mut NlContext, index: u32, value: f64) {
    if ctx.variable_is_locked[index as usize] {
        ctx.al.append(index, value);
    } else {
        ctx.af.append(ctx.variable_index[index as usize], value);
    }
}

pub fn nl_begin(ctx: &mut NlContext, prim: u32) {
    if prim == NL_SYSTEM {
        ctx.variable_value = vec![0.0; (ctx.nb_variables * ctx.nb_systems) as usize];
        ctx.variable_is_locked = vec![false; ctx.nb_variables as usize];
        ctx.variable_index = vec![0; ctx.nb_variables as usize];
    } else if prim == NL_MATRIX {
        if ctx.m.is_some() {
            return;
        }
        let mut n = 0u32;
        for i in 0..ctx.nb_variables {
            if !ctx.variable_is_locked[i as usize] {
                ctx.variable_index[i as usize] = n;
                n += 1;
            } else {
                ctx.variable_index[i as usize] = !0u32;
            }
        }
        ctx.n = n;
        if !ctx.max_iterations_defined {
            ctx.max_iterations = n * 5;
        }
        ctx.m = Some(Matrix::Sparse(SparseMatrix::new(n, n)));
        ctx.x = vec![0.0; (n * ctx.nb_systems) as usize];
        ctx.b = vec![0.0; (n * ctx.nb_systems) as usize];
        nl_variables_to_vector(ctx);
        ctx.af = NlRowColumn::default();
        ctx.al = NlRowColumn::default();
        ctx.current_row = 0;
    } else if prim == NL_ROW {
        ctx.af.zero();
        ctx.al.zero();
    }
}

pub fn nl_end(ctx: &mut NlContext, prim: u32) {
    if prim == NL_MATRIX {
        ctx.af = NlRowColumn::default();
        ctx.al = NlRowColumn::default();
    } else if prim == NL_ROW {
        let nf = ctx.af.coeff.len();
        let nl = ctx.al.coeff.len();
        let n = ctx.n;
        if let Some(Matrix::Sparse(m)) = ctx.m.as_mut() {
            for i in 0..nf {
                for j in 0..nf {
                    m.add(
                        ctx.af.coeff[i].index,
                        ctx.af.coeff[j].index,
                        ctx.af.coeff[i].value * ctx.af.coeff[j].value,
                    );
                }
            }
        }
        for k in 0..ctx.nb_systems {
            let mut s = 0.0;
            for jj in 0..nl {
                let j = ctx.al.coeff[jj].index;
                s += ctx.al.coeff[jj].value
                    * ctx.variable_value[(k * ctx.nb_variables + j) as usize];
            }
            if let Some(Matrix::Sparse(_)) = ctx.m.as_ref() {
                for jj in 0..nf {
                    ctx.b[(k * n + ctx.af.coeff[jj].index) as usize] -=
                        ctx.af.coeff[jj].value * s;
                }
            }
        }
        ctx.current_row += 1;
    }
}

pub fn nl_solve(ctx: &mut NlContext) -> bool {
    let p = match ctx.m.as_ref() {
        Some(Matrix::Sparse(m)) => jacobi_from_sparse(m),
        _ => return false,
    };
    ctx.p = Some(p);
    if let Some(Matrix::Sparse(sm)) = ctx.m.take() {
        ctx.m = Some(Matrix::Crs(CrsMatrix::from_sparse(&sm)));
    }
    let n = ctx.n as usize;
    for k in 0..ctx.nb_systems {
        let b_off = (k as usize) * n;
        let x_off = (k as usize) * n;
        let b = ctx.b[b_off..b_off + n].to_vec();
        let mut x = ctx.x[x_off..x_off + n].to_vec();
        if let (Some(m), Some(p)) = (ctx.m.as_ref(), ctx.p.as_ref()) {
            solve_pre_cg(m, p, &b, &mut x, ctx.threshold, ctx.max_iterations);
        }
        ctx.x[x_off..x_off + n].copy_from_slice(&x);
    }
    nl_vector_to_variables(ctx);
    true
}
