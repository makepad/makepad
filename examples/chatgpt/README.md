# Makepad Example: ChatGPT

A simple example of a Makepad Framework application that uses its network layer to interact with OpenAI's GPT model.

## How to Run

1. Set up your OpenAI API key:
   - Visit [OpenAI's API Key page](https://platform.openai.com/api-keys) and follow the instructions to create and retrieve your API key.
   - Note: Using the OpenAI API incurs costs.

2. Configure the API key as an environment variable:
   ```sh
   export OPENAI_API_KEY=your_openai_api_key
   ```

   Optionally, you can configure the model and endpoint:
   ```sh
   export OPENAI_MODEL=your_desired_model  # default is "gpt-4o"
   export OPENAI_BASE_URL=your_custom_api_base_url  # default is "https://api.openai.com/v1"
   ```

3. Run the application:
   ```sh
   cargo run
   ```