In this tutorial, we're going to build an image viewer app with Makepad.

Our image viewer app will support both an image grid and a slideshow view, with the ability to switch between the two. By the time you've reached the end of this tutorial, the image grid will look like this:
![[Image Filtering.png]]while the slideshow will look like this:
![[Slideshow 1.png]]
In addition, our app will provide a way to filter images based on a query string.

The goal of this tutorial is to 

## Setup for the Tutorial
Makepad is written in Rust, so we'll need to make sure Rust is installed on our system. The recommended way to install Rust is with Rustup, Rust's official toolchain installer. To install it, run the following command in your terminal, and follow the on-screen instructions.

```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```
## Tutorial Steps
The tutorial is divided into several steps. In each step, we'll implement a new feature. Each step builds on the previous one, so we recommend following them in order.

- [[Step 1 - Creating a Minimal Makepad App]]
- [[Step 2 - Creating a Static Image Grid]]
- [[Step 3 - Making the Image Grid Dynamic]]
- [[Step 4 - Creating a Slideshow]]
- [[Step 5 - Switching between Views]]
- [[Step 6 - Filtering Images]]
