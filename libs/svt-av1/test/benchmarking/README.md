# Test scripts for image and video codec comparison

# QUICK START
- `./setup.sh`
- `./run_comparison.sh configs/test_image_config.yaml`
- `./run_comparison.sh configs/test_video_config.yaml`

Outputs are in `~/benchmark/out`


# MORE INFO

### SETUP
Setup:
 `./setup.sh <benchmark_dir>`
This will install all the required libraries, setup a conda environment and activate it.


### CONFIG
Experiment setup should be provided through a config file. Example config files are provided under the configs directory.
In the config file generally the following fields only need to be modified:
`"root_dir", "out_dir", "tag", "dataset", "allowed_codecs", "allowed_metrics"`
For completeness, the following snippet shows a more comprehensive set of fields that might need modification, along with their explanation.

```
    placeholders:
        root_dir: "~/benchmark"                             # Points to the root directory of where the binary files and datasets are located
        out_dir: "~/benchmark/out"                          # Output directory for the results (encoded/decoded/summary results)
        tag: "AV2-CTC"                                      # Name used for output folders and log files

    dataset:
        source_dir: "{root_dir}/DataSet/AV2-CTC"            # Which dataset to use as input (will contain any of png/yuv/y4m files)
        tmp_dir: "{root_dir}/tmp"                           # Directory where different formats of the input dataset will be stored during (if input is png, then yuv and y4m will be generated, etc)

    codecs:
        allowed_codecs: ["jpegli", "svtav1"]                # List of codecs that should be used

    metrics:
        allowed_metrics: ["vmaf", "ssimulacra2", "ms_ssim"] # List of metrics to compute. PSNR is calculated via VMAF library
        allow_bdrate: true                                  # Set to true to enable BDRate metric computation by default
        anchor_encoder: "jpegli"                            # Encoder used to generate the reference for BDRate
        anchor_speed: 0                                     # Encoder speed for the reference BDRate computation
        aom_ctc_model: "v6.0"                               # AOM CTC model version

    settings:
        max_processes: 0                                    # 0 for auto scaling based on host configuration
        remove_decoded_files: true                          # Removes decoded files to reduce storage required to run benchmark
```


### RUN
Once you have the config file, you can run the experiment with:
 `./run_comparison.sh <config_file>`

This will generate all encodings using specified quality and speed values.
Then it will generate the decoded files and run the quality metric computation, storing the xml, ssimulacra2 results to file.
Lastly it'll generate the final report.

Encoding, decoding and summary maybe performed on separate machines, see summary.py for details.
