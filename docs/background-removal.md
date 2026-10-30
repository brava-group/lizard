# Remover o fundo de uma imagem

No aplicativo desktop, selecione uma camada com preenchimento de imagem e clique em **Remova o plano de fundo** na barra contextual. O Lizard processa a imagem localmente com o [withoutBG Open Weights](https://github.com/withoutbg/withoutbg-python), insere o PNG com transparência no mesmo preenchimento e mantém a escala, o recorte, as demais propriedades e o histórico de desfazer. Se a imagem ou o documento mudar durante o processamento, o resultado antigo é descartado.

Na primeira execução, o Lizard procura o SDK `withoutbg` no `python3` disponível. Se não estiver instalado, cria um ambiente Python em `~/.cache/lizard/withoutbg-venv` e instala a versão 1.3.0. O SDK baixa os pesos do modelo no cache do Hugging Face quando necessário. O download pode chegar a aproximadamente 1,5 GB; depois, a inferência roda sem enviar a imagem à API. É possível apontar `LIZARD_WITHOUTBG_PYTHON` para um Python com o SDK instalado e `WITHOUTBG_MODEL_PATH` para pesos ONNX locais.

O SDK é Apache-2.0. O modelo combina código e pesos do withoutBG com materiais DINOv3 sob a [licença da Meta](https://ai.meta.com/resources/models-and-libraries/dinov3-license/) e BiRefNet sob MIT. Built with DINOv3. Os pesos não fazem parte do repositório ou do binário do Lizard.
