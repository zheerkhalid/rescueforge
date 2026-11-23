//! Loads previews on a worker thread: thumbnails for the grid and the full
//! preview (picture, text or hex) for the selected file.

use crate::model::FoundFile;
use crate::preview::{self, Decoded};
use crate::source::Source;
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};

struct Job {
    id: u32,
    gen: u64,
    full: bool,
    file: FoundFile,
    src: Source,
}

struct Done {
    id: u32,
    gen: u64,
    full: bool,
    image: Option<Decoded>,
    head: Vec<u8>,
    /// Text taken out of the file by a format reader (documents, archives).
    text: Option<String>,
    caption: &'static str,
}

/// The preview of the selected file.
pub struct Full {
    pub id: u32,
    pub loading: bool,
    pub texture: Option<TextureHandle>,
    /// Size of the original picture.
    pub dims: (u32, u32),
    pub head: Vec<u8>,
    pub text: Option<String>,
    pub hex: String,
    /// What the preview shows when that is not obvious ("" otherwise).
    pub caption: &'static str,
}

type Queue = Arc<(Mutex<VecDeque<Job>>, Condvar)>;

pub struct Previewer {
    queue: Queue,
    rx: Receiver<Done>,
    /// Finished thumbnails; `None` means "cannot be shown".
    pub thumbs: HashMap<u32, Option<TextureHandle>>,
    requested: HashSet<u32>,
    pub full: Option<Full>,
    /// Bumped whenever file numbers change meaning (new scan, entries removed).
    pub gen: u64,
}

const HEAD_BYTES: usize = 64 * 1024;

impl Previewer {
    pub fn new(ctx: &egui::Context) -> Previewer {
        let queue: Queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let (tx, rx) = channel();
        let (q, ctx) = (queue.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("preview".into())
            .spawn(move || loop {
                let job = {
                    let (lock, cv) = &*q;
                    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if let Some(j) = guard.pop_front() {
                            break j;
                        }
                        guard = cv.wait(guard).unwrap_or_else(|e| e.into_inner());
                    }
                };
                let ext = job.file.ext();
                let size = job.file.size;
                let picture = preview::is_picture(&ext) && size <= preview::MAX_PICTURE_BYTES;
                let embedded = preview::has_embedded_picture(&ext);
                let mut text = None;
                let mut caption = "";
                let (image, head) = if job.full {
                    let reach = if picture {
                        size as usize
                    } else if embedded {
                        preview::embedded_reach(&ext, false)
                    } else if preview::is_zip_based(&ext) {
                        preview::ZIP_REACH
                    } else {
                        HEAD_BYTES
                    };
                    let mut all = preview::head(&job.src, &job.file, reach.max(HEAD_BYTES));
                    let image = if picture {
                        preview::decode_picture(&all, 1400)
                    } else if embedded {
                        let found = preview::embedded_picture(&all, 1400);
                        if found.is_some() {
                            caption = "The preview picture stored inside this file";
                        }
                        found
                    } else {
                        None
                    };
                    if preview::is_zip_based(&ext) {
                        if let Some((t, c)) = preview::zip_preview(&all) {
                            text = Some(t);
                            caption = c;
                        }
                    }
                    all.truncate(HEAD_BYTES);
                    (image, all)
                } else if picture {
                    let all = preview::head(&job.src, &job.file, size as usize);
                    (preview::decode_picture(&all, 180), Vec::new())
                } else if embedded {
                    let all =
                        preview::head(&job.src, &job.file, preview::embedded_reach(&ext, true));
                    (preview::embedded_picture(&all, 180), Vec::new())
                } else {
                    (None, Vec::new())
                };
                if tx
                    .send(Done {
                        id: job.id,
                        gen: job.gen,
                        full: job.full,
                        image,
                        head,
                        text,
                        caption,
                    })
                    .is_err()
                {
                    break;
                }
                ctx.request_repaint();
            })
            .expect("preview thread");
        Previewer {
            queue,
            rx,
            thumbs: HashMap::new(),
            requested: HashSet::new(),
            full: None,
            gen: 0,
        }
    }

    /// Forgets everything (the file numbers are about to change).
    pub fn reset(&mut self) {
        self.gen += 1;
        self.thumbs.clear();
        self.requested.clear();
        self.full = None;
        let (lock, _) = &*self.queue;
        lock.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    fn push(&mut self, job: Job, front: bool) {
        let queue = self.queue.clone();
        let (lock, cv) = &*queue;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        if front {
            q.push_front(job);
        } else {
            // Thumbnails scrolled past long ago are not worth decoding.
            while q.len() > 120 {
                if let Some(old) = q.iter().position(|j| !j.full) {
                    if let Some(j) = q.remove(old) {
                        // It may be asked for again later.
                        self.requested.remove(&j.id);
                    }
                } else {
                    break;
                }
            }
            q.push_back(job);
        }
        cv.notify_one();
    }

    /// Asks for a thumbnail unless it is already there or on its way.
    pub fn want_thumb(&mut self, id: u32, file: &FoundFile, src: &Source) {
        if self.thumbs.contains_key(&id) || !self.requested.insert(id) {
            return;
        }
        if self.requested.len() > 4000 {
            self.requested.clear();
        }
        self.push(
            Job {
                id,
                gen: self.gen,
                full: false,
                file: file.clone(),
                src: src.clone(),
            },
            false,
        );
    }

    /// Makes `id` the file shown in the preview panel.
    pub fn want_full(&mut self, id: u32, file: &FoundFile, src: Option<&Source>) {
        if self.full.as_ref().map(|f| f.id) == Some(id) {
            return;
        }
        self.full = Some(Full {
            id,
            loading: src.is_some(),
            texture: None,
            dims: (0, 0),
            head: Vec::new(),
            text: None,
            hex: String::new(),
            caption: "",
        });
        if let Some(src) = src {
            self.push(
                Job {
                    id,
                    gen: self.gen,
                    full: true,
                    file: file.clone(),
                    src: src.clone(),
                },
                true,
            );
        }
    }

    /// Takes finished work from the worker. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(done) = self.rx.try_recv() {
            if done.gen != self.gen {
                continue;
            }
            let texture = done.image.as_ref().map(|d| {
                let img = ColorImage::from_rgba_unmultiplied(
                    [d.width as usize, d.height as usize],
                    &d.rgba,
                );
                ctx.load_texture(
                    format!("preview-{}-{}", done.id, done.full),
                    img,
                    TextureOptions::LINEAR,
                )
            });
            if done.full {
                if let Some(full) = self.full.as_mut().filter(|f| f.id == done.id) {
                    full.loading = false;
                    full.texture = texture;
                    full.dims = done.image.as_ref().map(|d| d.full).unwrap_or((0, 0));
                    full.caption = done.caption;
                    full.text = done
                        .text
                        .or_else(|| preview::as_text(&done.head))
                        .or_else(|| preview::as_legacy_text(&done.head));
                    full.hex = preview::hex_dump(&done.head, 512);
                    full.head = done.head;
                }
            } else {
                if self.thumbs.len() > 1500 {
                    self.thumbs.clear();
                    self.requested.clear();
                }
                self.thumbs.insert(done.id, texture);
            }
        }
    }
}
